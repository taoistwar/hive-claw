fn main() {
    let date_str = "2026-07-01 11:46:00";

    let naive = match chrono::NaiveDateTime::parse_from_str(date_str, "%Y-%m-%d %H:%M:%S") {
        Ok(dt) => dt,
        Err(_) => {
            return;
        }
    };
    println!("naive: {}", naive);
    // 强制解释为东8区，转为 UTC 后与 DB 比较
    let offset = chrono::FixedOffset::east_opt(8 * 3600).expect("UTC+8 offset");
    let cutoff = naive
        .and_local_timezone(offset)
        .single()
        .map(|dt| dt.naive_utc())
        .unwrap_or(naive);
    println!("cutoff: {}", cutoff);
    println!("cutoff timestamp: {}", cutoff.and_utc().timestamp());
    let eq = 1782877560 == cutoff.and_utc().timestamp();
    println!("{eq}");
}
