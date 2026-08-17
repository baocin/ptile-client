//! Records that are an airport's internal plumbing rather than places.
//!
//! An OSM-derived business layer carries a departure board. Around Nashville
//! and Memphis the ground is thick with nodes named `AA 1445 BNA-LAX`,
//! `DL 1656 - BNA to DTW`, `Delta Flight 973 - MCI to ATL`, `Gate B12`,
//! `Concourse C4`. None of them is somewhere a person goes: a flight is an
//! event and a gate is a doorway inside a building you have already arrived
//! at. They crowd out the real businesses near an airport and they are never
//! what a search meant.
//!
//! This lives in the library rather than in one client so every consumer --
//! the Android app, the wasm demo, anything reading a pack directly -- sees
//! the same layer. [`crate::business`] applies it at decode, so packs already
//! on disk come back clean without being rebuilt; `scripts/build_full_ptilesb.py`
//! applies the same rule at build time, so new packs never carry them at all.
//! **The two implementations have to agree**: `ptiles/flightnodes.py` is the
//! mirror of this file and its tests use the same fixtures.
//!
//! Measured against the published `TN.business.ptiles` (829,528 named
//! records) this drops 1,234 names, and they cluster on BNA, MEM, TYS and CHA
//! -- which is the evidence it is not catching anything else. It is the
//! weaker half of the rule: the records belong to one category, and the names
//! recognise only 922 of the 1,710 records in it. The builder drops the whole
//! category, which the client cannot do -- a pack carries a category *index*,
//! renumbered per state and even between builds of the same state, and never
//! a label. Proximity to an airport is deliberately *not* used: no builder emits
//! an aeroway layer, so there is nothing to measure against, and 45 of the
//! names caught are flights logged nowhere near one.

use alloc::string::String;

/// Airline designators that begin a flight number.
///
/// A closed list, because the letters are the only thing separating `DL 1656`
/// from `BAS 128`. Accepting any two or three capitals -- which an earlier
/// client-side rule did -- deleted 174 real places in Tennessee alone: `BAC
/// 41`, `AMB 210`, `HWY 385`, `ACT 1`, `FOX 16`.
const CARRIERS: [&str; 21] = [
    "AA", "DL", "UA", "WN", "AS", "B6", "F9", "NK", "G4", "HA", "SY", "AC", "WS", "9E", "OO", "YX",
    "MQ", "QX", "ZW", "EV", "YV",
];

/// Airlines written out in full, as they appear before `Flight 973`.
const AIRLINE_WORDS: [&str; 13] = [
    "delta",
    "american",
    "united",
    "southwest",
    "alaska",
    "jetblue",
    "frontier",
    "spirit",
    "allegiant",
    "hawaiian",
    "envoy",
    "republic",
    "skywest",
];

/// Words that name a piece of airside furniture rather than a destination.
const AIRSIDE_WORDS: [&str; 6] = ["gate", "concourse", "stand", "apron", "terminal", "pier"];

/// Drop every record whose name says it is a flight or a gate.
///
/// Applied where records become *results*, not inside the decoders: v4 has no
/// per-record framing, so `records.len()` against the index's `feature_count`
/// is the only cheap signal that a block desynchronised, and a decoder that
/// quietly returns fewer records than the index promised would break that
/// check on every cell containing an airport.
pub fn drop_flight_nodes<T>(records: &mut alloc::vec::Vec<T>, name_of: impl Fn(&T) -> &str) {
    records.retain(|record| !is_flight_node(name_of(record)));
}

/// True when a record's name says it is a flight or a gate, not a place.
pub fn is_flight_node(name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return false;
    }
    is_designator(trimmed) || has_spelled_flight(trimmed) || is_airside(trimmed)
}

/// `DL3208`, `AA 1087`, `UA6157 To DEN`, `AA 2926 CHA/DFW Seat 11A`.
///
/// Whatever follows the number is ignored rather than described: it is free
/// text -- a route, a seat, a note -- and the designator alone is decisive.
/// What is *not* ignored is the boundary after the number, so `HWY 55 Burgers`
/// and `US 43 Drag Raceway` never reach this in the first place (their letters
/// are not carriers) and a name like `AA 12Th Street Diner` does not match,
/// because the character after the digits is a letter rather than a break.
fn is_designator(name: &str) -> bool {
    let upper = to_upper(name);
    for carrier in CARRIERS {
        let Some(rest) = upper.strip_prefix(carrier) else {
            continue;
        };
        // The carrier code has to be a word of its own: `ASHLAND` starts with
        // `AS` and is a town.
        if rest.starts_with(|c: char| c.is_ascii_alphanumeric()) && !rest.starts_with(char::is_numeric) {
            continue;
        }
        // `DL Flight # 5437 MEM to IAH`: the code, the word, then the number.
        let worded = rest.trim_start();
        if let Some(after) = worded.strip_prefix("FLIGHT") {
            let digits = after.trim_start_matches([' ', '#', '-']);
            if digits.starts_with(|c: char| c.is_ascii_digit()) {
                return true;
            }
        }
        // `DL BNA->ATL`, `AA MEM to DFW`: a flight named by its route with no
        // number at all. Anchored on the carrier, so real names built from two
        // capitalised abbreviations -- `JAN-PRO`, `AVI-SPL`, `POW-MIA` -- are
        // untouched, and `DL Cabinetry` has no route to find.
        if has_airport_pair(worded) {
            return true;
        }
        // `DL3208`, `AA 1087`, `DL #956`, `DL - 2435 (Memphis - New York LGA)`
        // are all the same thing written four ways. What may sit between the
        // code and the number is only spacing and a flight-number marker; a
        // letter here means this is a different word and not a designator.
        let rest = rest.trim_start_matches([' ', '#', '-']);
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 || digits > 4 {
            continue;
        }
        // An optional single suffix letter (`DL 123A`), then the name must
        // stop or break.
        let after = &rest[digits..];
        let after = match after.chars().next() {
            Some(c) if c.is_ascii_alphabetic() && after.len() == 1 => "",
            _ => after,
        };
        match after.chars().next() {
            None => return true,
            Some(c) if !c.is_ascii_alphanumeric() => return true,
            _ => {}
        }
    }
    false
}

/// Two three-letter airport codes joined by a route marker: `BNA->ATL`,
/// `MEM to IAH`, `CHA/DFW`, `BNA - LAX`.
///
/// Expects an already-uppercased string.
fn has_airport_pair(upper: &str) -> bool {
    let bytes = upper.as_bytes();
    let is_code = |at: usize| -> bool {
        at + 3 <= bytes.len()
            && bytes[at..at + 3].iter().all(u8::is_ascii_uppercase)
            && bytes.get(at + 3).is_none_or(|b| !b.is_ascii_alphabetic())
            && (at == 0 || !bytes[at - 1].is_ascii_alphabetic())
    };
    for at in 0..bytes.len() {
        if !is_code(at) {
            continue;
        }
        let rest = &upper[at + 3..];
        let joined = ["->", ">", "-", "/", " TO ", " – ", "–", " > ", " - "]
            .iter()
            .find_map(|marker| rest.strip_prefix(*marker).or_else(|| {
                rest.trim_start().strip_prefix(*marker)
            }));
        let Some(after) = joined else { continue };
        let after = after.trim_start();
        if after.len() >= 3 && after.as_bytes()[..3].iter().all(u8::is_ascii_uppercase) {
            return true;
        }
    }
    false
}

/// `Delta Flight 973 - MCI to ATL`, `American Airlines Flight 1221`.
fn has_spelled_flight(name: &str) -> bool {
    let lower = to_lower(name);
    let Some(at) = find(&lower, "flight ") else {
        return false;
    };
    // A number has to follow, or this is `Flight Deck Bar`.
    if !lower[at + "flight ".len()..]
        .trim_start()
        .starts_with(|c: char| c.is_ascii_digit())
    {
        return false;
    }
    // And an airline has to precede it, or this is `Flight 93 Memorial`.
    let before = &lower[..at];
    AIRLINE_WORDS.iter().any(|airline| find(before, airline).is_some())
}

/// `Gate 5`, `Gate B12`, `Terminal 2`, `Concourse C4`.
///
/// The whole name must be the word and its number. That is what keeps `Gate
/// Communications` and `Gateway Tire` -- both real businesses -- out of it.
fn is_airside(name: &str) -> bool {
    let lower = to_lower(name);
    for word in AIRSIDE_WORDS {
        let Some(rest) = lower.strip_prefix(word) else {
            continue;
        };
        // The word has to end here: `gateway` is not `gate`.
        let rest = rest.trim_start_matches([' ', '-', '#']);
        if rest.len() == lower.len() - word.len() && !rest.is_empty() {
            // Nothing was trimmed and something follows: `gateway`.
            if !rest.starts_with(|c: char| c.is_ascii_digit() || c.is_ascii_alphabetic()) {
                continue;
            }
            if lower.as_bytes().get(word.len()).is_some_and(|b| b.is_ascii_alphabetic())
                && !is_bay_label(rest)
            {
                continue;
            }
        }
        if is_bay_label(rest) {
            return true;
        }
    }
    false
}

/// `5`, `12`, `B7`, `C20`, `A3` -- an optional letter around 1..=3 digits.
fn is_bay_label(rest: &str) -> bool {
    let body = rest.strip_prefix(|c: char| c.is_ascii_alphabetic()).unwrap_or(rest);
    let digits = body.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 3 {
        return false;
    }
    let tail = &body[digits..];
    tail.is_empty() || (tail.len() == 1 && tail.starts_with(|c: char| c.is_ascii_alphabetic()))
}

fn to_upper(s: &str) -> String {
    s.chars().map(|c| c.to_ascii_uppercase()).collect()
}

fn to_lower(s: &str) -> String {
    s.chars().map(|c| c.to_ascii_lowercase()).collect()
}

/// `str::find` for a substring, spelled out so this stays no_std-clean.
fn find(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    (0..=haystack.len() - needle.len())
        .filter(|i| haystack.is_char_boundary(*i))
        .find(|&i| haystack[i..].starts_with(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every name here was read out of the published `TN.business.ptiles`.
    #[test]
    fn designators_are_flights_with_or_without_their_route() {
        for name in [
            "DL3208",
            "AA 1087",
            "DL4795",
            "AA 1445 BNA-LAX",
            "DL 1656 - BNA to DTW",
            "UA6157 To DEN",
            "AA3908 BNA - ORD",
            "AA 2903 CHA/DFW",
            "AA 2999 (TYS > ORD)",
            "AA 3027 BNA>ORD",
            "AA 2926 CHA/DFW Seat 11A",
            "AA 1735 MEM to DFW Non-stop",
            // Read out of TN.business_name_index.ptiles, where a typed search
            // for "DL" found them and this rule first did not.
            "DL #956",
            "DL - 2435 (Memphis - New York LGA)",
            "DL 1088 MEM To ATL",
            "DL 1230 (Atlanta To Minneapolis)",
            // No number at all, or the word instead of the code.
            "DL BNA->ATL",
            "DL BNA->MSP",
            "DL Flight # 5437 MEM to IAH",
            "DL Flight #4968 TRI to ATL",
        ] {
            assert!(is_flight_node(name), "{name}");
        }
    }

    #[test]
    fn spelled_out_flights_count_too() {
        assert!(is_flight_node("Delta Flight 973 - MCI to ATL"));
        assert!(is_flight_node("American Airlines Flight 1221"));
        assert!(is_flight_node("delta flight 2323"));
    }

    /// A flight number needs an airline, and an airline needs a flight number.
    #[test]
    fn neither_word_alone_is_enough() {
        assert!(!is_flight_node("Flight 93 Memorial"));
        assert!(!is_flight_node("Flight Deck Bar & Grill"));
        assert!(!is_flight_node("Delta Dental of Tennessee"));
        assert!(!is_flight_node("United Grocery Outlet"));
    }

    /// The 174 real places an any-two-capitals rule deleted in Tennessee.
    #[test]
    fn highways_and_unit_numbers_are_places() {
        for name in [
            "HWY 54", "HWY 385", "HWY 45N", "US 51", "TN0106", "BAC 41", "BAC 45", "BAS 128",
            "AMB 210", "AMG 116", "ACT 1", "ASP 2011", "ABC24", "FOX 16", "OR 7", "PT2", "MW3",
            "KU4K",
        ] {
            assert!(!is_flight_node(name), "{name}");
        }
    }

    #[test]
    fn businesses_carrying_a_number_survive() {
        for name in [
            "HWY 55 Burgers",
            "US 43 Drag Raceway",
            "ONE9 Travel Center",
            "FPC 731 Lexington",
            "HWY 191 Recycling & Auto Salvage",
            "VFW 4840 - Ray Pinner Post, Tipton County, TN",
            // A carrier code is a common word opening: none of these is a
            // flight, and two of them are three capitals joined by a dash.
            "DL Cabinetry",
            "JAN-PRO Cleaning & Disinfecting",
            "AVI-SPL",
            "POW-MIA Wall Clifton Tn",
            "CTG-IFC Disposables",
            "Ashland City Hardware",
        ] {
            assert!(!is_flight_node(name), "{name}");
        }
    }

    #[test]
    fn gates_and_concourses_are_not_destinations() {
        for name in ["Gate 5", "Gate 12", "Gate B7", "Gate C20", "gate a3", "Terminal 2"] {
            assert!(is_flight_node(name), "{name}");
        }
    }

    /// The trailing number is what separates a gate from a company.
    #[test]
    fn a_business_named_after_a_gate_is_kept() {
        for name in ["Gate Communications", "Gateway Tire", "Golden Gate Cafe", "Terminal Brewhouse"]
        {
            assert!(!is_flight_node(name), "{name}");
        }
    }

    #[test]
    fn an_empty_name_is_not_a_flight() {
        assert!(!is_flight_node(""));
        assert!(!is_flight_node("   "));
    }
}
