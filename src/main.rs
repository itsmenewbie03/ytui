use color_eyre::Result;

mod app;
mod config;
pub mod player;
pub mod scraper;
mod spectrum;
mod sponsorblock;

fn main() -> Result<()> {
    color_eyre::install()?;
    app::run()
}
