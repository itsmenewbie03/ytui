use color_eyre::Result;

mod app;
pub mod player;
pub mod scraper;

fn main() -> Result<()> {
    color_eyre::install()?;
    app::run()
}
