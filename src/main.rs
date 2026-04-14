use anyhow::Result;
use tracing::info;

mod blacklist;
mod config;
mod lidarr;
mod scorer;
mod slskd;
mod sync;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "kagurarr=info".into()),
        )
        .init();

    let config_path =
        std::env::var("KAGURARR_CONFIG").unwrap_or_else(|_| "/data/config.toml".into());
    let cfg = config::load(&config_path)?;

    info!("kagurarr starting up");
    info!("lidarr: {}", cfg.lidarr.url);
    info!("slskd:  {}", cfg.slskd.url);
    info!("interval: {}s, min_score: {}", cfg.kagurarr.interval_secs, cfg.kagurarr.min_score);

    let db = blacklist::Blacklist::open(&cfg.database.path)?;
    let lidarr = lidarr::LidarrClient::new(&cfg.lidarr.url, &cfg.lidarr.api_key);
    let slskd = slskd::SlskdClient::new(&cfg.slskd.url, &cfg.slskd.api_key);

    loop {
        if let Err(e) = sync::run_cycle(&cfg, &db, &lidarr, &slskd).await {
            tracing::error!("cycle error: {e:#}");
        }

        info!("sleeping {}s until next cycle", cfg.kagurarr.interval_secs);
        tokio::time::sleep(std::time::Duration::from_secs(cfg.kagurarr.interval_secs)).await;
    }
}
