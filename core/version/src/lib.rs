pub mod notifier {
    use std::time::Duration;
    use tokio::time;
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    pub async fn check_release() -> Result<Option<String>, self_update::errors::Error> {
        log::trace!("Market release checker started");
        let releases = self_update::backends::github::ReleaseList::configure()
            .repo_owner("golemfactory")
            .repo_name("yagna")
            .build()?
            .fetch()?;
        for r in releases {
            if self_update::version::bump_is_greater(VERSION, r.version.as_str())? {
                return Ok(Some(r.version))
            }
        }
        log::trace!("Market release checker done");
        Ok(None)
    }

    pub async fn worker() {
        let mut interval = time::interval(Duration::from_secs(3600 * 24));
        loop {
            interval.tick().await;
            if let Ok(Some(version)) = check_release().await.or_else::<self_update::errors::Error, Option<String>>(|e| {
                log::debug!("Problem encountered during checking for new releases: {}", e);
                None
            }) {
                log::info!("New version of yagna available {}", version);
                // Drop previous DB entries
                // Insert new DB entry (version, false-seen flag)
            };
        }
    }

    pub async fn pinger() {
        let mut interval = time::interval(Duration::from_secs(60 * 24));
        loop {
            interval.tick().await;
            let version = "9.9.9";
            let seen = false;
            if !seen {
                log::warn!("New version of yagna available {}", version);
            }
        };
    }
}

mod test {
    use anyhow::Result;

    async fn test_check_release() -> Result<()> {
        let result = crate::notifier::check_release().await?;
        println!("Check version result: {:?}", result);
        Ok(())
    }
}
