//! User preference for routine background-tab unloading.

use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_MINUTES: u8 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdleSetting {
    /// None disables routine unloading. macOS memory pressure can still free tabs.
    pub minutes: Option<u8>,
}

impl Default for IdleSetting {
    fn default() -> Self {
        Self {
            minutes: Some(DEFAULT_MINUTES),
        }
    }
}

impl IdleSetting {
    fn path() -> PathBuf {
        crate::blocker::data_dir().join("idle-unload.txt")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text == "off" {
            return Some(Self { minutes: None });
        }
        let minutes = text.parse::<u8>().ok().filter(|m| (1..=60).contains(m))?;
        Some(Self {
            minutes: Some(minutes),
        })
    }

    pub fn duration(self) -> Option<Duration> {
        self.minutes.map(|m| Duration::from_secs(u64::from(m) * 60))
    }

    pub fn save(self) {
        let value = self
            .minutes
            .map_or_else(|| "off".to_string(), |m| m.to_string());
        let path = Self::path();
        if let Err(e) = std::fs::create_dir_all(crate::blocker::data_dir())
            .and_then(|_| std::fs::write(path, value))
        {
            crate::log::log(&format!("save idle setting failed: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_off_and_one_to_sixty_minutes() {
        assert_eq!(IdleSetting::parse("off").unwrap().duration(), None);
        assert_eq!(
            IdleSetting::parse("30").unwrap().duration(),
            Some(Duration::from_secs(1800))
        );
        assert_eq!(IdleSetting::parse("60").unwrap().minutes, Some(60));
        assert!(IdleSetting::parse("0").is_none());
        assert!(IdleSetting::parse("61").is_none());
    }
}
