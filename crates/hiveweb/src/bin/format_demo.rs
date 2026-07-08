fn main() {
    let tag_id = 1;
    let tag_json = format!("{{\"type\": {}}}", tag_id);
    println!("{}", tag_json);
}
