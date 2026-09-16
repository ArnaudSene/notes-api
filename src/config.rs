use std::collections::HashMap;
use std::fmt;

/// The environment the service refused to start on. `Display` never
/// includes a value read from the environment: none of the variables this
/// error can be about are safe to echo back except `PORT`, and even that is
/// not worth the special case.
#[derive(Debug, PartialEq, Eq)]
pub struct ConfigError(String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ConfigError {}

/// The three variables the service reads, and nothing else.
pub struct Config {
    pub token: String,
    pub database_url: Option<String>,
    pub port: u16,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("token", &"<redacted>")
            .field("database_url", &self.database_url)
            .field("port", &self.port)
            .finish()
    }
}

const DEFAULT_PORT: u16 = 8080;

impl Config {
    /// Reads the process environment. The only entry point production code
    /// uses; tests go through [`Config::from_vars`] instead, so that
    /// checking a missing or malformed variable never touches the real
    /// environment of the process running the suite.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_vars(std::env::vars())
    }

    pub fn from_vars(vars: impl Iterator<Item = (String, String)>) -> Result<Self, ConfigError> {
        let vars: HashMap<String, String> = vars.collect();

        let token = match vars.get("NOTES_TOKEN") {
            Some(token) if !token.is_empty() => token.clone(),
            _ => {
                return Err(ConfigError(
                    "NOTES_TOKEN is required and must not be empty".to_string(),
                ));
            }
        };

        let database_url = vars.get("DATABASE_URL").cloned();

        let port = match vars.get("PORT") {
            None => DEFAULT_PORT,
            Some(raw) => raw.parse().map_err(|_| {
                ConfigError(format!("PORT must be a 16-bit port number, got {raw:?}"))
            })?,
        };

        Ok(Config {
            token,
            database_url,
            port,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> impl Iterator<Item = (String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn reads_the_token() {
        let config = Config::from_vars(vars(&[("NOTES_TOKEN", "secret")])).unwrap();

        assert_eq!(config.token, "secret");
    }

    #[test]
    fn missing_token_is_an_error() {
        let result = Config::from_vars(vars(&[]));

        assert!(result.is_err());
    }

    #[test]
    fn empty_token_is_an_error() {
        let result = Config::from_vars(vars(&[("NOTES_TOKEN", "")]));

        assert!(result.is_err());
    }

    #[test]
    fn error_message_never_carries_the_token() {
        let result = Config::from_vars(vars(&[("NOTES_TOKEN", "")]));

        let message = result.unwrap_err().to_string();
        assert!(!message.contains("secret"));
        assert!(message.to_lowercase().contains("notes_token"));
    }

    #[test]
    fn database_url_is_absent_when_unset() {
        let config = Config::from_vars(vars(&[("NOTES_TOKEN", "secret")])).unwrap();

        assert_eq!(config.database_url, None);
    }

    #[test]
    fn database_url_is_read_when_set() {
        let config = Config::from_vars(vars(&[
            ("NOTES_TOKEN", "secret"),
            ("DATABASE_URL", "postgres://db/notes"),
        ]))
        .unwrap();

        assert_eq!(config.database_url, Some("postgres://db/notes".to_string()));
    }

    #[test]
    fn port_defaults_to_8080() {
        let config = Config::from_vars(vars(&[("NOTES_TOKEN", "secret")])).unwrap();

        assert_eq!(config.port, 8080);
    }

    #[test]
    fn port_is_read_when_set() {
        let config =
            Config::from_vars(vars(&[("NOTES_TOKEN", "secret"), ("PORT", "3000")])).unwrap();

        assert_eq!(config.port, 3000);
    }

    #[test]
    fn port_that_does_not_parse_is_an_error() {
        let result = Config::from_vars(vars(&[("NOTES_TOKEN", "secret"), ("PORT", "not-a-port")]));

        assert!(result.is_err());
    }

    #[test]
    fn port_out_of_u16_range_is_an_error() {
        let result = Config::from_vars(vars(&[("NOTES_TOKEN", "secret"), ("PORT", "99999")]));

        assert!(result.is_err());
    }
}
