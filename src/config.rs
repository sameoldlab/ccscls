//! User settings, read from the client's `initializationOptions`.
//!
//! Anything unreadable falls back to the defaults, and the problem is
//! reported to the caller instead of failing: a typo in an editor config must
//! not stop the server.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Off,
    Hint,
    Information,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// A class no stylesheet defines.
    pub undefined_severity: Severity,
    /// A class defined somewhere, but not linked from the file that uses it.
    pub ambient_severity: Severity,
    /// Also report in files whose CSS sources are not fully known (fragments,
    /// pages with scripts, external stylesheets). Off by default.
    pub report_in_open_world: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            undefined_severity: Severity::Warning,
            ambient_severity: Severity::Hint,
            report_in_open_world: false,
        }
    }
}

impl Config {
    /// Read `initializationOptions`. Returns the settings and, if the options
    /// could not be read, a description of the problem (the defaults are used).
    pub fn from_options(options: Option<&serde_json::Value>) -> (Self, Option<String>) {
        let Some(options) = options.filter(|value| !value.is_null()) else {
            return (Self::default(), None);
        };
        match serde_json::from_value::<Self>(options.clone()) {
            Ok(config) => (config, None),
            Err(error) => (Self::default(), Some(error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_apply_when_nothing_is_given() {
        let expected = Config::default();
        assert_eq!(Config::from_options(None), (expected.clone(), None));
        assert_eq!(
            Config::from_options(Some(&json!(null))),
            (expected.clone(), None)
        );
        assert_eq!(Config::from_options(Some(&json!({}))), (expected, None));
    }

    #[test]
    fn known_options_override_and_unknown_ones_are_ignored() {
        let options = json!({
            "ambientSeverity": "warning",
            "undefinedSeverity": "error",
            "reportInOpenWorld": true,
            "somethingElse": 1,
        });
        let (config, problem) = Config::from_options(Some(&options));
        assert_eq!(problem, None);
        assert_eq!(config.ambient_severity, Severity::Warning);
        assert_eq!(config.undefined_severity, Severity::Error);
        assert!(config.report_in_open_world);
    }

    #[test]
    fn a_partial_object_keeps_the_other_defaults() {
        let (config, problem) = Config::from_options(Some(&json!({ "ambientSeverity": "off" })));
        assert_eq!(problem, None);
        assert_eq!(config.ambient_severity, Severity::Off);
        assert_eq!(
            config.undefined_severity,
            Config::default().undefined_severity
        );
    }

    #[test]
    fn unreadable_options_fall_back_and_explain() {
        for options in [
            json!({ "ambientSeverity": "loud" }),
            json!("text"),
            json!({ "reportInOpenWorld": "yes" }),
        ] {
            let (config, problem) = Config::from_options(Some(&options));
            assert_eq!(config, Config::default(), "options = {options}");
            assert!(problem.is_some(), "options = {options}");
        }
    }
}
