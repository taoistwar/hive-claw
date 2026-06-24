use hivegui::model::tools::{ToolSeries, ToolSeriesKind};

#[test]
fn hour_plus_one_ships_empty() {
    let s = ToolSeries::for_kind(ToolSeriesKind::HourPlusOne);
    assert!(s.tools.is_empty());
    assert_eq!(s.display_name_zh(), "Hour+1 工具");
}
