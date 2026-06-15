pub fn unix_utc_2024_01_01() -> i64 {
    1_704_067_200
}

pub fn parse_fixture_date(value: &str) -> Option<i64> {
    match value.trim() {
        "2024-01-01" => Some(unix_utc_2024_01_01()),
        "2024-02-01" => Some(1_706_745_600),
        _ => None,
    }
}

pub fn parse_ymd(value: &str) -> Option<i64> {
    let mut parts = value.trim().split(['-', '/']);
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400)
}

pub fn parse_ymd_from_path(path: &str) -> Option<i64> {
    let mut parts = path.trim_start_matches('/').split('/');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - (month <= 2) as i32;
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let yoe = year - era * 400;
    let mp = month as i32 + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) as i64
}
