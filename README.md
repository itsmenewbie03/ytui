# ytui 🎧 [![author/maintainer](https://img.shields.io/badge/by-itsmenewbie03-00bcd4.svg?logo=github&labelColor=181717&longCache=true&style=flat-square)](https://itsmenewbie03.github.io/)

**ytui** is a keyboard-driven YouTube Music client that keeps browsing, search, and playback inside your terminal.

Like this project? **Leave a star**! ⭐⭐⭐⭐⭐

> [!NOTE]
> ytui is under active development and is not affiliated with YouTube or Google.

## ✨ Features

- Browse YouTube Music home shelves
- Sign in with a browser cookie for personalized account data
- Search for songs, videos, albums, artists, and playlists
- Play read-only playlists end to end, then hand off to Automix
- Build interactive Up Next queues from YouTube Music Automix
- Play the highest-quality available audio stream through `mpv`
- Pause, seek, and move through the current queue
- Visualize playback with audio-reactive spectrum bars
- Control playback from desktop media keys and MPRIS clients
- See playback progress, duration, views, and likes at a glance
- Navigate with Vim-style keys or arrow keys
- Copy playback diagnostics with an available system clipboard tool
- Automatically skip selected community-reported segments with SponsorBlock
- Follow syllable-synced, line-synced, or plain lyrics in the full player

The full player includes tabs for lyrics, the queue, comments, and related tracks. Lyrics and the queue are ready today; comments and related tracks are friendly placeholders for what comes next.

## 🗺️ Planned Features

- **Mouse support:** enable clicking tabs, controls, navigation items, and tracks while preserving the keyboard-first workflow.

## ⚙️ Requirements

You must have the following available on your system:

- Linux with PipeWire
- A recent [Rust toolchain](https://www.rust-lang.org/tools/install) with Cargo
- [`mpv`](https://mpv.io/) on your `PATH`
- A [Nerd Font](https://www.nerdfonts.com/) for the transport icons
- An internet connection

Building requires the PipeWire development files (`pipewire` on Arch Linux or `libpipewire-0.3-dev` on Debian and Ubuntu).

Clipboard support is optional. Install `wl-copy`, `xclip`, or `xsel` if you want to copy playback diagnostics from the app.

On Linux desktops, ytui automatically publishes playback metadata and controls over MPRIS when a D-Bus session is available. This supports media keys, lock-screen controls, `playerctl`, Waybar, KDE, and GNOME without an mpv plugin, including the current track's artwork.

## 🛠️ Setup

Clone the repository and enter the project directory:

```shell
git clone https://github.com/itsmenewbie03/ytui.git
cd ytui
```

ytui does not need an account, API key, or configuration file to get started.

## 🔨 Building

Build an optimized binary with Cargo:

```shell
cargo build --release
```

The binary will be available at `target/release/ytui`.

## 🚀 Running

Launch ytui directly from the repository:

```shell
cargo run --release
```

Or install it locally and run it from your shell:

```shell
cargo install --path .
ytui
```

Once it opens, choose a home item or press `/`, type a search, and press `Enter` to start listening.

## 🔐 Optional Sign-In

Open **Settings**, select **YouTube Music Account**, and press `Enter` to import a Netscape-format `cookies.txt` file. The recommended export flow follows [yt-dlp's YouTube guidance](https://github.com/yt-dlp/yt-dlp/wiki/Extractors#exporting-youtube-cookies):

1. Install [`cookies.txt`](https://addons.mozilla.org/en-US/firefox/addon/cookies-txt/) for Firefox or [`Get cookies.txt LOCALLY`](https://chromewebstore.google.com/detail/get-cookiestxt-locally/cclelndahbckbenkjhflpdbgdldlbecc) for Chromium. Allow the extension in private windows.
2. Open exactly one private window and tab, then sign into YouTube.
3. In that same tab, visit `https://www.youtube.com/robots.txt`.
4. Export only `youtube.com` cookies in Netscape format, then immediately close the private window and do not reuse that session.
5. Enter the exported file path in ytui. Paths beginning with `~/` are supported.

ytui keeps only cookies applicable to `music.youtube.com`, validates the account, and saves the resulting cookie header to `$XDG_CONFIG_HOME/ytui/credentials` with owner-only permissions on Unix. Delete the exported `cookies.txt` afterward. Press `c` on the account setting if you need the previous raw `Cookie` request-header fallback.

Treat both files like passwords: the cookies grant access to your YouTube account. Press `d` on the account setting to remove the local credential and return to an anonymous session. Be careful with similarly named browser extensions: yt-dlp specifically warns against the old **Get cookies.txt** extension, which was reported as malware.

### 🎥 Optional Watch History Sync

When signed in, open **Settings**, move to **Playback**, and toggle **Sync watch history** (`h` / `l` / `Enter`) to report plays to your YouTube Music account. This keeps your watch history and personalized recommendations in sync. It is off by default, and playback otherwise remains anonymous.

### ⏭️ Optional SponsorBlock Skipping

Open **Settings**, select **SponsorBlock** under **Playback**, and enable the segment categories you want ytui to skip automatically. Every category is off by default. When at least one category is enabled, playback sends the current YouTube video ID to the public [SponsorBlock](https://sponsor.ajay.app/) service; YouTube account cookies are never included.

### 🎤 Lyrics

When a track starts, ytui searches [BiniLyrics](https://lyrics-api.binimum.org/), [Unison](https://unison.boidu.dev/), and [LRCLIB](https://lrclib.net/) using its title, artist, and any available album and duration metadata; Unison is also queried by the current YouTube video ID for exact matches. YouTube account cookies are never sent to these services. The Lyrics tab prefers syllable-synced lyrics, then line-synced lyrics, and finally plain text; songwriter and provider attribution appears after the lyrics when available.

### 🎶 Playlist Playback

Selecting a playlist starts it from its first track, exactly like YouTube Music Web. Playlists are read-only: ytui loads the full track list (including paginated playlists), plays tracks in order, and shows the remaining tracks in the Up Next queue. When the playlist's final track ends, an Automix seeded from that last track keeps the music going.

Playlists can be opened from a **Home** shelf or a **Search** result. Mix cards such as **Listen Again** keep their web behavior: pressing `Enter` starts the featured track immediately with its generated mix queue.

## ⌨️ Controls

### 🧭 Navigation

| Key | Action |
| --- | --- |
| `j` / `k` or `Down` / `Up` | Move through navigation and lists |
| `h` / `l` or `Left` / `Right` | Change shelf, tab, or focus |
| `Enter` | Open a section or play the selected item |
| `/` | Open search input |
| `Esc` | Cancel input, return to navigation, or leave the player |
| `P` | Open or close the full player |
| `q` | Quit |

In Settings, use `j` / `k` to select an option, `h` / `l` to change it, `Enter` to open the selected option, `c` to paste a raw cookie header, and `d` to remove a saved account cookie. In the **Playback** section, `h` / `l` / `Enter` toggle watch history sync and the compact mini-player layout. Inside SponsorBlock settings, use `j` / `k` to select a category and `Space` / `Enter` to toggle it. The standard mini player also collapses automatically when the terminal becomes narrow.

### 🎵 Playback

| Key | Action |
| --- | --- |
| `Space` | Pause or resume |
| `[` / `]` | Seek backward or forward 10 seconds |
| `p` / `n` | Play the previous or next queue item |
| `h` / `l`, `Tab` / `Shift+Tab` | Change full-player tab |

## 🧪 Development

Run the standard checks before sending a change:

```shell
cargo fmt --check
cargo check --all-targets
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

The live stream compatibility test is ignored by default because it needs network access and a working `mpv` installation. Run it explicitly with:

```shell
cargo test --test client_streams -- --ignored --nocapture
```

The MPRIS smoke test needs `dbus-run-session` and `playerctl`:

```shell
dbus-run-session -- cargo test registers_with_a_session_bus -- --ignored --nocapture
```

## 🔌 How It Works

ytui uses [`innertube-rs`](https://crates.io/crates/innertube-rs) to load YouTube Music data and resolve audio streams. Playback runs in a managed `mpv` process controlled through its JSON IPC socket, while [Ratatui](https://ratatui.rs/) and Crossterm power the terminal interface. CPAL captures the active PipeWire sink and `rustfft` turns its PCM samples into the player spectrum. A native MPRIS service bridges desktop media controls into the same playback state and command loop.

## 🤝 Contributing

Have an improvement in mind? [Open a pull request](https://github.com/itsmenewbie03/ytui/pulls). I welcome thoughtful fixes and features, and I will be glad to review them.

## 🐛 Issues

Found a bug or rough edge that has not already been reported? [Open an issue](https://github.com/itsmenewbie03/ytui/issues/new) and share what happened. Thanks for helping make ytui better!
