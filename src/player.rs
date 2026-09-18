use serde_json::{Value, json};
use std::{
    fs,
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Child, ChildStderr, Command, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::Duration,
};

pub enum PlayerEvent {
    FileLoaded,
    Position(f64),
    Duration(f64),
    Paused(bool),
    EndOfFile,
    Diagnostic(String),
    Error(String),
}

pub struct MpvPlayer {
    child: Child,
    command_stream: UnixStream,
    events: Receiver<PlayerEvent>,
    socket_path: PathBuf,
}

pub fn copy_to_clipboard(text: &str) -> io::Result<()> {
    let commands: [(&str, &[&str]); 3] = [
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    let mut last_error = None;

    for (program, args) in commands {
        let mut child = match Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                last_error = Some(error);
                continue;
            }
            Err(error) => return Err(error),
        };

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes())?;
        }
        let status = child.wait()?;
        if status.success() {
            return Ok(());
        }
        last_error = Some(io::Error::other(format!("{program} exited with {status}")));
    }

    Err(last_error.unwrap_or_else(|| io::Error::other("no clipboard command is available")))
}

impl MpvPlayer {
    pub fn start(url: &str) -> io::Result<Self> {
        let socket_path =
            std::env::temp_dir().join(format!("ytui-mpv-{}.sock", std::process::id()));
        let _ = fs::remove_file(&socket_path);

        let mut child = Command::new("mpv")
            .arg("--idle=yes")
            .arg("--no-video")
            .arg("--input-terminal=no")
            .arg("--msg-level=all=warn")
            .arg(format!("--input-ipc-server={}", socket_path.display()))
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;

        let command_stream = match connect(&mut child, &socket_path) {
            Ok(stream) => stream,
            Err(error) => {
                stop_child(&mut child, &socket_path);
                return Err(error);
            }
        };
        let reader_stream = match command_stream.try_clone() {
            Ok(stream) => stream,
            Err(error) => {
                stop_child(&mut child, &socket_path);
                return Err(error);
            }
        };
        let (event_sender, events) = mpsc::channel();
        let stderr = child.stderr.take();
        let stderr_sender = event_sender.clone();
        thread::spawn(move || read_events(reader_stream, event_sender));
        if let Some(stderr) = stderr {
            thread::spawn(move || read_stderr(stderr, stderr_sender));
        }

        let mut player = Self {
            child,
            command_stream,
            events,
            socket_path,
        };
        player.command(json!(["observe_property", 1, "time-pos"]))?;
        player.command(json!(["observe_property", 2, "duration"]))?;
        player.command(json!(["observe_property", 3, "pause"]))?;
        player.command(json!(["request_log_messages", "warn"]))?;

        Ok(player)
    }

    pub fn toggle_pause(&mut self) -> io::Result<()> {
        self.command(json!(["cycle", "pause"]))
    }

    pub fn seek(&mut self, seconds: i64) -> io::Result<()> {
        self.command(json!(["seek", seconds, "relative", "exact"]))
    }

    pub fn try_recv(&self) -> Result<PlayerEvent, TryRecvError> {
        self.events.try_recv()
    }

    fn command(&mut self, command: Value) -> io::Result<()> {
        serde_json::to_writer(&mut self.command_stream, &json!({ "command": command }))
            .map_err(io::Error::other)?;
        self.command_stream.write_all(b"\n")?;
        self.command_stream.flush()
    }
}

impl Drop for MpvPlayer {
    fn drop(&mut self) {
        let _ = self.command(json!(["quit"]));
        thread::sleep(Duration::from_millis(50));
        stop_child(&mut self.child, &self.socket_path);
    }
}

fn stop_child(child: &mut Child, socket_path: &PathBuf) {
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
    let _ = fs::remove_file(socket_path);
}

fn connect(child: &mut Child, socket_path: &PathBuf) -> io::Result<UnixStream> {
    for _ in 0..50 {
        if let Some(status) = child.try_wait()? {
            return Err(io::Error::other(format!(
                "mpv exited before IPC was ready: {status}"
            )));
        }
        match UnixStream::connect(socket_path) {
            Ok(stream) => return Ok(stream),
            Err(_) => thread::sleep(Duration::from_millis(20)),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "timed out waiting for mpv IPC socket",
    ))
}

fn read_events(stream: UnixStream, sender: mpsc::Sender<PlayerEvent>) {
    for line in BufReader::new(stream).lines().map_while(Result::ok) {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        let event = match message.get("event").and_then(Value::as_str) {
            Some("file-loaded") => Some(PlayerEvent::FileLoaded),
            Some("end-file") => match message.get("reason").and_then(Value::as_str) {
                Some("eof") => Some(PlayerEvent::EndOfFile),
                Some("stop" | "quit" | "redirect") => None,
                Some(reason) => {
                    let detail = message
                        .get("file_error")
                        .and_then(Value::as_str)
                        .unwrap_or(reason);
                    Some(PlayerEvent::Error(format!("playback ended: {detail}")))
                }
                None => None,
            },
            Some("log-message") => {
                let prefix = message
                    .get("prefix")
                    .and_then(Value::as_str)
                    .unwrap_or("mpv");
                let level = message
                    .get("level")
                    .and_then(Value::as_str)
                    .unwrap_or("warn");
                message
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(|text| {
                        PlayerEvent::Diagnostic(format!(
                            "[{prefix}/{level}] {}",
                            redact_stream_url(text)
                        ))
                    })
            }
            Some("property-change") => property_event(&message),
            _ => message
                .get("error")
                .and_then(Value::as_str)
                .filter(|error| *error != "success")
                .map(|error| PlayerEvent::Error(error.to_owned())),
        };

        if event.is_some_and(|event| sender.send(event).is_err()) {
            break;
        }
    }
}

fn read_stderr(stderr: ChildStderr, sender: mpsc::Sender<PlayerEvent>) {
    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
        let message = line.trim();
        if !message.is_empty()
            && sender
                .send(PlayerEvent::Diagnostic(redact_stream_url(message)))
                .is_err()
        {
            break;
        }
    }
}

fn property_event(message: &Value) -> Option<PlayerEvent> {
    let name = message.get("name")?.as_str()?;
    let data = message.get("data")?;
    match name {
        "time-pos" => data.as_f64().map(PlayerEvent::Position),
        "duration" => data.as_f64().map(PlayerEvent::Duration),
        "pause" => data.as_bool().map(PlayerEvent::Paused),
        _ => None,
    }
}

fn redact_stream_url(message: &str) -> String {
    message
        .split_whitespace()
        .map(|part| {
            if part.contains("googlevideo.com") || part.contains("videoplayback?") {
                "<stream-url>"
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
