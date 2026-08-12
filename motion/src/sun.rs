//! Sunrise and sunset, for a coordinate and a moment.
//!
//! Daylight is motion context. The same speed means different things at noon
//! and at midnight, a walk that starts an hour before sunset is a different
//! proposition from one that starts at dawn, and a sampling policy that knows
//! the sun is down can reason about a phone in a pocket rather than a phone in
//! a hand. So this sits beside the classifier rather than in the map layers:
//! it answers a question about the traveller, not about the ground.
//!
//! The algorithm is the standard low-precision solar position model (NOAA's,
//! the same one the Astronomical Almanac publishes for hand calculation).
//! Accurate to about a minute at temperate latitudes, degrading near the
//! poles, where the answer is usually "the sun does not rise today" anyway.
//! No ephemeris table, no timezone database, no network: a `no_std` crate
//! cannot afford any of those, and none is needed for a horizon crossing.
//!
//! Everything is UTC seconds since the epoch. Local time is a presentation
//! problem and needs a timezone database this crate does not have.

use ptiles_core::math::{atan2, cos, round, sin, sqrt};

/// Seconds in a mean solar day.
const DAY_S: f64 = 86_400.0;

/// Unix epoch as a Julian day number.
const UNIX_EPOCH_JD: f64 = 2_440_587.5;

/// J2000.0, the epoch the solar model is expressed against.
const J2000: f64 = 2_451_545.0;

/// Obliquity of the ecliptic, degrees. Drifts ~0.013 degrees a century, which
/// is far below this model's own error.
const OBLIQUITY_DEG: f64 = 23.4397;

/// Geometric sunrise: the sun's upper limb on the horizon, with refraction.
///
/// The centre of the disc is 50 arcminutes below the horizon at the moment the
/// limb appears to touch it -- 16' of semidiameter and 34' of atmospheric
/// refraction. Every almanac uses this figure.
pub const ELEVATION_SUNRISE_DEG: f64 = -0.833;

/// Civil twilight: enough light to see without artificial help.
pub const ELEVATION_CIVIL_DEG: f64 = -6.0;

/// Nautical twilight: the horizon is still discernible at sea.
pub const ELEVATION_NAUTICAL_DEG: f64 = -12.0;

/// When the sun crosses a given elevation, either side of solar noon.
///
/// `rise` and `set` are `None` together when the sun stays above or below that
/// elevation for the whole day -- polar summer and polar winter, and for the
/// twilight elevations a good deal further from the poles than people expect.
/// [`SunTimes::sun_up`] says which of the two it was.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SunTimes {
    pub rise_unix_s: Option<i64>,
    pub set_unix_s: Option<i64>,
    /// Always defined: the sun has a highest point even when it never sets.
    pub solar_noon_unix_s: i64,
    /// True when the sun is above the elevation at solar noon.
    ///
    /// With no rise and no set this is the whole answer: true is a day that
    /// never ends, false is a night that never lifts.
    pub sun_up: bool,
}

impl SunTimes {
    /// Seconds between rise and set, or `None` on a day with neither.
    pub fn daylight_s(&self) -> Option<i64> {
        match (self.rise_unix_s, self.set_unix_s) {
            (Some(rise), Some(set)) => Some(set - rise),
            _ => None,
        }
    }
}

/// Sunrise and sunset for the day containing `unix_s`, at `lat`/`lon`.
pub fn sun_times(lat_deg: f64, lon_deg: f64, unix_s: i64) -> SunTimes {
    sun_times_at(lat_deg, lon_deg, unix_s, ELEVATION_SUNRISE_DEG)
}

/// Is the sun above the horizon at this instant?
///
/// Compares against the horizon crossings of the day the instant falls in, so
/// it stays correct across the midnight either side of it.
pub fn is_daylight(lat_deg: f64, lon_deg: f64, unix_s: i64) -> bool {
    let times = sun_times(lat_deg, lon_deg, unix_s);
    match (times.rise_unix_s, times.set_unix_s) {
        (Some(rise), Some(set)) => unix_s >= rise && unix_s <= set,
        _ => times.sun_up,
    }
}

/// Seconds until the sun sets, or `None` when it will not set today.
///
/// Negative once it has set: a caller deciding whether to suggest turning back
/// wants the sign, not an absolute value.
pub fn seconds_to_sunset(lat_deg: f64, lon_deg: f64, unix_s: i64) -> Option<i64> {
    sun_times(lat_deg, lon_deg, unix_s).set_unix_s.map(|set| set - unix_s)
}

/// The general form: when the sun crosses `elevation_deg`.
///
/// Pass [`ELEVATION_CIVIL_DEG`] for the light a walker can still navigate by,
/// which is the number that matters on a trail rather than sunset itself.
pub fn sun_times_at(lat_deg: f64, lon_deg: f64, unix_s: i64, elevation_deg: f64) -> SunTimes {
    let lat = lat_deg.to_radians();

    // Days since J2000, shifted west so that a "day" is local to this
    // longitude: solar noon is what the whole calculation hangs off, and it
    // is a local event.
    let julian_day = unix_s as f64 / DAY_S + UNIX_EPOCH_JD;
    let n = round(julian_day - J2000 - 0.0009 + lon_deg / 360.0);
    let mean_solar_noon = n + 0.0009 - lon_deg / 360.0;

    // Mean anomaly, then the equation of centre: the Earth's orbit is an
    // ellipse, so the sun runs early in January and late in July.
    let mean_anomaly_deg = (357.5291 + 0.985_600_28 * mean_solar_noon) % 360.0;
    let mean_anomaly = mean_anomaly_deg.to_radians();
    let centre_deg = 1.9148 * sin(mean_anomaly)
        + 0.0200 * sin(2.0 * mean_anomaly)
        + 0.0003 * sin(3.0 * mean_anomaly);

    // Ecliptic longitude, and from it the declination -- how far north or
    // south of the equator the sun stands today.
    let ecliptic_deg = (mean_anomaly_deg + centre_deg + 180.0 + 102.9372) % 360.0;
    let ecliptic = ecliptic_deg.to_radians();
    let declination = asin(sin(ecliptic) * sin(OBLIQUITY_DEG.to_radians()));

    // Solar transit, corrected for the two effects that make a sundial
    // disagree with a clock.
    let transit_jd =
        J2000 + mean_solar_noon + 0.0053 * sin(mean_anomaly) - 0.0069 * sin(2.0 * ecliptic);
    let solar_noon_unix_s = julian_to_unix(transit_jd);

    // The hour angle at which the sun sits at the requested elevation. Out of
    // range means the sun never gets there: the horizon crossing does not
    // exist today.
    let numerator = sin(elevation_deg.to_radians()) - sin(lat) * sin(declination);
    let denominator = cos(lat) * cos(declination);
    let sun_up_at_noon = sin(lat) * sin(declination) + cos(lat) * cos(declination)
        > sin(elevation_deg.to_radians());
    if denominator == 0.0 {
        return SunTimes {
            rise_unix_s: None,
            set_unix_s: None,
            solar_noon_unix_s,
            sun_up: sun_up_at_noon,
        };
    }
    let cos_hour_angle = numerator / denominator;
    if !(-1.0..=1.0).contains(&cos_hour_angle) {
        return SunTimes {
            rise_unix_s: None,
            set_unix_s: None,
            solar_noon_unix_s,
            sun_up: cos_hour_angle < -1.0,
        };
    }

    let hour_angle_deg = acos(cos_hour_angle).to_degrees();
    let half_day = hour_angle_deg / 360.0;
    SunTimes {
        rise_unix_s: Some(julian_to_unix(transit_jd - half_day)),
        set_unix_s: Some(julian_to_unix(transit_jd + half_day)),
        solar_noon_unix_s,
        sun_up: true,
    }
}

fn julian_to_unix(jd: f64) -> i64 {
    round((jd - UNIX_EPOCH_JD) * DAY_S) as i64
}

/// `asin` and `acos` are not in core's math shim, and adding them there would
/// mean touching every `no_std` arm for two callers. Both follow from `atan2`
/// and `sqrt`, which the shim already routes through libm.
fn asin(x: f64) -> f64 {
    let x = x.clamp(-1.0, 1.0);
    atan2(x, sqrt(1.0 - x * x))
}

fn acos(x: f64) -> f64 {
    let x = x.clamp(-1.0, 1.0);
    atan2(sqrt(1.0 - x * x), x)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-06-21T12:00:00Z, the June solstice.
    const JUNE_SOLSTICE: i64 = 1_782_043_200;
    /// 2026-12-21T12:00:00Z, the December solstice.
    const DECEMBER_SOLSTICE: i64 = 1_797_854_400;
    /// 2026-03-20T12:00:00Z, near the March equinox.
    const EQUINOX: i64 = 1_774_008_000;

    const LONDON: (f64, f64) = (51.5074, -0.1278);
    const QUITO: (f64, f64) = (-0.1807, -78.4678);
    const LONGYEARBYEN: (f64, f64) = (78.2232, 15.6267);
    const SYDNEY: (f64, f64) = (-33.8688, 151.2093);

    fn minutes_between(a: i64, b: i64) -> f64 {
        (a - b) as f64 / 60.0
    }

    /// London on the June solstice: sunrise 03:43 UTC, sunset 20:21 UTC, as
    /// published. Ten minutes of slack for a model that claims about one.
    #[test]
    fn london_midsummer_matches_the_published_times() {
        let times = sun_times(LONDON.0, LONDON.1, JUNE_SOLSTICE);
        let rise = times.rise_unix_s.expect("the sun rises in London");
        let set = times.set_unix_s.expect("and sets");

        // 2026-06-21T03:43Z and T20:21Z.
        assert!(minutes_between(rise, 1_782_013_380).abs() < 10.0, "sunrise {rise}");
        assert!(minutes_between(set, 1_782_073_260).abs() < 10.0, "sunset {set}");
    }

    /// The equator gets twelve hours of daylight whatever the season -- the
    /// one place the answer is known without an almanac.
    #[test]
    fn the_equator_gets_twelve_hours_all_year() {
        for moment in [JUNE_SOLSTICE, DECEMBER_SOLSTICE, EQUINOX] {
            let daylight = sun_times(QUITO.0, QUITO.1, moment)
                .daylight_s()
                .expect("the sun rises on the equator every day");
            let hours = daylight as f64 / 3_600.0;
            assert!((hours - 12.0).abs() < 0.35, "{hours} hours at {moment}");
        }
    }

    /// Above the Arctic circle the sun does not set in June or rise in
    /// December, and the two cases must not look alike to a caller.
    #[test]
    fn polar_day_and_polar_night_are_distinguishable() {
        let summer = sun_times(LONGYEARBYEN.0, LONGYEARBYEN.1, JUNE_SOLSTICE);
        assert_eq!(summer.rise_unix_s, None);
        assert_eq!(summer.set_unix_s, None);
        assert!(summer.sun_up, "midnight sun");
        assert!(is_daylight(LONGYEARBYEN.0, LONGYEARBYEN.1, JUNE_SOLSTICE));

        let winter = sun_times(LONGYEARBYEN.0, LONGYEARBYEN.1, DECEMBER_SOLSTICE);
        assert_eq!(winter.rise_unix_s, None);
        assert!(!winter.sun_up, "polar night");
        assert!(!is_daylight(LONGYEARBYEN.0, LONGYEARBYEN.1, DECEMBER_SOLSTICE));
        assert_eq!(winter.daylight_s(), None);
    }

    /// Seasons invert across the equator: a solstice that is longest in London
    /// is shortest in Sydney.
    #[test]
    fn the_southern_hemisphere_runs_the_other_way() {
        let london_june = sun_times(LONDON.0, LONDON.1, JUNE_SOLSTICE).daylight_s().unwrap();
        let sydney_june = sun_times(SYDNEY.0, SYDNEY.1, JUNE_SOLSTICE).daylight_s().unwrap();
        let sydney_december =
            sun_times(SYDNEY.0, SYDNEY.1, DECEMBER_SOLSTICE).daylight_s().unwrap();

        assert!(london_june > 16 * 3_600, "London midsummer is a long day");
        assert!(sydney_june < 10 * 3_600, "and Sydney's midwinter is a short one");
        assert!(sydney_december > sydney_june);
    }

    #[test]
    fn sunrise_precedes_solar_noon_precedes_sunset() {
        let times = sun_times(LONDON.0, LONDON.1, JUNE_SOLSTICE);

        assert!(times.rise_unix_s.unwrap() < times.solar_noon_unix_s);
        assert!(times.solar_noon_unix_s < times.set_unix_s.unwrap());
    }

    /// Twilight brackets the day: light before the sun clears the horizon and
    /// after it drops below.
    #[test]
    fn civil_twilight_is_wider_than_the_day_itself() {
        let day = sun_times(LONDON.0, LONDON.1, EQUINOX);
        let civil = sun_times_at(LONDON.0, LONDON.1, EQUINOX, ELEVATION_CIVIL_DEG);

        assert!(civil.rise_unix_s.unwrap() < day.rise_unix_s.unwrap());
        assert!(civil.set_unix_s.unwrap() > day.set_unix_s.unwrap());
        assert!(civil.daylight_s().unwrap() > day.daylight_s().unwrap());
    }

    /// The signed answer is the point: a walker wants to know they are already
    /// half an hour late, not that sunset is half an hour away.
    #[test]
    fn time_to_sunset_goes_negative_once_it_has_set() {
        let times = sun_times(LONDON.0, LONDON.1, JUNE_SOLSTICE);
        let set = times.set_unix_s.unwrap();

        assert!(seconds_to_sunset(LONDON.0, LONDON.1, set - 1_800).unwrap() > 0);
        assert!(seconds_to_sunset(LONDON.0, LONDON.1, set + 1_800).unwrap() < 0);
    }

    /// Every instant of a day resolves to the same rise and set, whichever
    /// side of noon it falls -- the west shift is what makes this hold.
    #[test]
    fn any_moment_in_a_day_gives_that_days_crossings() {
        let noon = sun_times(LONDON.0, LONDON.1, JUNE_SOLSTICE);
        for offset in [-6 * 3_600, -3_600, 3_600, 6 * 3_600] {
            let other = sun_times(LONDON.0, LONDON.1, JUNE_SOLSTICE + offset);
            assert_eq!(other.rise_unix_s, noon.rise_unix_s, "offset {offset}");
            assert_eq!(other.set_unix_s, noon.set_unix_s, "offset {offset}");
        }
    }

    #[test]
    fn daylight_tracks_the_crossings_it_reports() {
        let times = sun_times(LONDON.0, LONDON.1, EQUINOX);
        let rise = times.rise_unix_s.unwrap();
        let set = times.set_unix_s.unwrap();

        assert!(!is_daylight(LONDON.0, LONDON.1, rise - 600));
        assert!(is_daylight(LONDON.0, LONDON.1, rise + 600));
        assert!(is_daylight(LONDON.0, LONDON.1, set - 600));
        assert!(!is_daylight(LONDON.0, LONDON.1, set + 600));
    }
}
