use anyhow::Result;
use tracing::info;

mod blacklist;
mod config;
mod lidarr;
mod scorer;
mod slskd;

#[tokio::main]
async fn main() -> Result<()> {
    // set up logging before we do anything else
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "kagurarr=info".into()),
        )
        .init();

    let config_path = std::env::var("KAGURARR_CONFIG").unwrap_or_else(|_| "/data/config.toml".into());
    let cfg = config::load(&config_path)?;

    info!("kagurarr starting up");
    info!("lidarr: {}", cfg.lidarr.url);
    info!("slskd: {}", cfg.slskd.url);

    let db = blacklist::Blacklist::open(&cfg.database.path)?;
    let lidarr = lidarr::LidarrClient::new(&cfg.lidarr.url, &cfg.lidarr.api_key);
    let slskd = slskd::SlskdClient::new(&cfg.slskd.url, &cfg.slskd.api_key);

    loop {
        if let Err(e) = run_cycle(&cfg, &db, &lidarr, &slskd).await {
            tracing::error!("cycle failed: {e:#}");
        }

        info!("sleeping for {}s", cfg.kagurarr.interval_secs);
        tokio::time::sleep(std::time::Duration::from_secs(cfg.kagurarr.interval_secs)).await;
    }
}

async fn run_cycle(
    cfg: &config::Config,
    db: &blacklist::Blacklist,
    lidarr: &lidarr::LidarrClient,
    slskd: &slskd::SlskdClient,
) -> Result<()> {
    let wanted = lidarr.wanted_albums().await?;
    info!("found {} wanted album(s)", wanted.len());

    // TODO: implement the actual sync loop
    // for each wanted album:
    //   search slskd
    //   score candidates
    //   filter blacklisted
    //   download best match
    //   poll until done
    //   trigger lidarr import
    //   blacklist on success or failure

    let _ = (cfg, db, slskd); // silence unused warnings until implemented
    Ok(())
}
