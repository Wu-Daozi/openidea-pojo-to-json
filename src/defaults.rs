//! Type-name based default values for JSON skeletons.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// Today's date and the current time at one UTC offset.
///
/// Computed once per command so every date/time field in one JSON document
/// shows the same instant. The plugin sandbox has no access to the host's
/// timezone, so the UTC offset comes from the plugin setting.
#[derive(Clone, Debug)]
pub(crate) struct Now {
    pub(crate) date: String,
    pub(crate) time: String,
    pub(crate) date_time: String,
}

impl Now {
    pub(crate) fn from_offset_minutes(offset_minutes: i64) -> Self {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);
        Self::from_unix_seconds(seconds, offset_minutes)
    }

    fn from_unix_seconds(seconds: i64, offset_minutes: i64) -> Self {
        let local = seconds + offset_minutes * 60;
        let days = local.div_euclid(86_400);
        let seconds_of_day = local.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        let hour = seconds_of_day / 3_600;
        let minute = (seconds_of_day % 3_600) / 60;
        let second = seconds_of_day % 60;

        let date = format!("{year:04}-{month:02}-{day:02}");
        let time = format!("{hour:02}:{minute:02}:{second:02}");
        let date_time = format!("{date} {time}");
        Now {
            date,
            time,
            date_time,
        }
    }

    #[cfg(test)]
    pub(crate) fn epoch() -> Self {
        Self::from_unix_seconds(0, 0)
    }
}

/// A scalar default for a well-known Java type, or `None` when the type is
/// not recognized (the caller then treats it as an object or `null`).
pub(crate) fn default_scalar(name: &str, now: &Now) -> Option<Value> {
    Some(match name {
        "boolean" | "Boolean" | "AtomicBoolean" => json!(false),

        "byte" | "short" | "int" | "long" | "Byte" | "Short" | "Integer" | "Long"
        | "BigInteger" | "BigDecimal" | "Number" | "AtomicInteger" | "AtomicLong" => json!(0),

        "float" | "double" | "Float" | "Double" => json!(0.0),

        "char" | "Character" | "String" | "CharSequence" | "StringBuilder" | "StringBuffer" => {
            json!("")
        }

        "UUID" => json!("00000000-0000-0000-0000-000000000000"),

        "LocalDate" => json!(now.date),
        "LocalTime" | "Time" => json!(now.time),
        "LocalDateTime" | "Date" | "Timestamp" | "Instant" | "ZonedDateTime" | "OffsetDateTime" => {
            json!(now.date_time)
        }
        "Duration" => json!("PT0S"),
        "Period" => json!("P0D"),

        _ => return None,
    })
}

/// Well-known JDK types that have no useful field skeleton. They must not
/// trigger a project lookup just because a same-named source class exists.
pub(crate) fn is_opaque(name: &str) -> bool {
    matches!(
        name,
        "Object"
            | "Class"
            | "Enum"
            | "Serializable"
            | "Comparable"
            | "Cloneable"
            | "AutoCloseable"
            | "Throwable"
    )
}

/// Collection-like types serialize as `[]`.
pub(crate) fn is_list(name: &str) -> bool {
    matches!(
        name,
        "Iterable"
            | "Collection"
            | "List"
            | "ArrayList"
            | "LinkedList"
            | "Vector"
            | "Stack"
            | "Set"
            | "HashSet"
            | "LinkedHashSet"
            | "TreeSet"
            | "SortedSet"
            | "NavigableSet"
            | "Queue"
            | "Deque"
            | "ArrayDeque"
    )
}

/// Map-like types serialize as `{}`.
pub(crate) fn is_map(name: &str) -> bool {
    matches!(
        name,
        "Map"
            | "HashMap"
            | "LinkedHashMap"
            | "TreeMap"
            | "SortedMap"
            | "NavigableMap"
            | "Hashtable"
            | "ConcurrentHashMap"
            | "Properties"
    )
}

/// `Optional` types serialize as `null`.
pub(crate) fn is_optional(name: &str) -> bool {
    matches!(
        name,
        "Optional" | "OptionalInt" | "OptionalLong" | "OptionalDouble"
    )
}

/// Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = (z - era * 146_097) as u64; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_scalars() {
        let now = Now::epoch();
        assert_eq!(default_scalar("int", &now), Some(json!(0)));
        assert_eq!(default_scalar("Integer", &now), Some(json!(0)));
        assert_eq!(default_scalar("boolean", &now), Some(json!(false)));
        assert_eq!(default_scalar("String", &now), Some(json!("")));
        assert_eq!(
            default_scalar("UUID", &now),
            Some(json!("00000000-0000-0000-0000-000000000000"))
        );
        assert_eq!(default_scalar("LocalDate", &now), Some(json!("1970-01-01")));
        assert_eq!(default_scalar("LocalTime", &now), Some(json!("00:00:00")));
        assert_eq!(
            default_scalar("LocalDateTime", &now),
            Some(json!("1970-01-01 00:00:00"))
        );
        assert_eq!(default_scalar("Nope", &now), None);
    }

    #[test]
    fn classifies_containers() {
        assert!(is_list("List"));
        assert!(is_list("ArrayList"));
        assert!(!is_list("Map"));
        assert!(is_map("HashMap"));
        assert!(is_map("Properties"));
        assert!(is_optional("Optional"));
        assert!(!is_optional("String"));
    }

    #[test]
    fn formats_offsets_and_known_dates() {
        assert_eq!(Now::from_unix_seconds(0, 0).date, "1970-01-01");
        assert_eq!(
            Now::from_unix_seconds(0, 480).date_time,
            "1970-01-01 08:00:00"
        );
        assert_eq!(
            Now::from_unix_seconds(1_700_000_000, 0).date_time,
            "2023-11-14 22:13:20"
        );
        assert_eq!(
            Now::from_unix_seconds(-1, 0).date_time,
            "1969-12-31 23:59:59"
        );
    }
}
