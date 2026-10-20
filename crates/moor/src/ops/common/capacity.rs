//! The global Box limit from the user config, and whether it is reached.

use std::num::NonZeroU32;

use moor_core::config::UserConfig;
use moor_core::decide::capacity::{admit, Admission, CapacityFacts, HeldBox};
use moor_core::ProviderId;

use crate::store::BoxRow;

/// The configured Box limit and the config key it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Limit {
    /// The most Boxes the provider may hold.
    pub(crate) max: NonZeroU32,
    /// The config key the limit was read from.
    pub(crate) key: String,
}

/// The Box limit for `provider`: `drivers.<provider>.max_boxes`, else `box.max_boxes`.
pub(crate) fn limit(config: &UserConfig, provider: &str) -> Option<Limit> {
    if let Some(max) = config.drivers.get(provider).and_then(|d| d.max_boxes) {
        return Some(Limit {
            max,
            key: format!("drivers.{provider}.max_boxes"),
        });
    }
    config.box_.max_boxes.map(|max| Limit {
        max,
        key: "box.max_boxes".to_owned(),
    })
}

/// `Some(limit)` when every slot for the provider is taken by `holders`; `None` when one is free.
pub(crate) fn full(
    config: &UserConfig,
    provider: &ProviderId,
    holders: Vec<BoxRow>,
) -> Option<Limit> {
    let limit = limit(config, provider.as_str())?;
    let boxes = holders
        .into_iter()
        .filter_map(|row| {
            row.boxed.map(|b| HeldBox {
                holder: row.name.as_str().to_owned(),
                state: b.state,
            })
        })
        .collect();
    let facts = CapacityFacts {
        limit: Some(limit.max),
        boxes,
    };
    match admit(&facts) {
        Admission::Admitted => None,
        Admission::AtCapacity { .. } => Some(limit),
    }
}

#[cfg(test)]
mod tests {
    use moor_core::config::{BoxConfig, DriverConfig};

    use super::*;

    fn n(v: u32) -> NonZeroU32 {
        NonZeroU32::new(v).unwrap()
    }

    fn config(box_max: Option<u32>, drivers: &[(&str, u32)]) -> UserConfig {
        let mut c = UserConfig {
            box_: BoxConfig {
                max_boxes: box_max.map(n),
                ..BoxConfig::default()
            },
            ..UserConfig::default()
        };
        for (name, max) in drivers {
            c.drivers.insert(
                (*name).to_owned(),
                DriverConfig {
                    max_boxes: Some(n(*max)),
                    lookup_timeout_secs: None,
                    strip_env: Vec::new(),
                },
            );
        }
        c
    }

    #[test]
    fn driver_key_wins_over_box_key() {
        let got = limit(&config(Some(5), &[("local", 2)]), "local");
        assert_eq!(
            got,
            Some(Limit {
                max: n(2),
                key: "drivers.local.max_boxes".into()
            })
        );
    }

    #[test]
    fn box_key_used_when_driver_key_absent() {
        let got = limit(&config(Some(5), &[]), "local");
        assert_eq!(
            got,
            Some(Limit {
                max: n(5),
                key: "box.max_boxes".into()
            })
        );
    }

    #[test]
    fn other_providers_driver_entry_is_ignored() {
        let got = limit(&config(Some(5), &[("cloud", 1)]), "local");
        assert_eq!(
            got,
            Some(Limit {
                max: n(5),
                key: "box.max_boxes".into()
            })
        );
    }

    #[test]
    fn none_when_neither_is_set() {
        assert_eq!(limit(&config(None, &[]), "local"), None);
    }
}
