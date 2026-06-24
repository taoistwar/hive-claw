//! v1 ships zero tools in both series (FR-013, FR-014); invariant S1.

use hivegui::model::tools::{ToolSeries, ToolSeriesKind};

#[test]
fn day_plus_one_ships_empty() {
    let s = ToolSeries::for_kind(ToolSeriesKind::DayPlusOne);
    assert!(s.tools.is_empty());
    assert_eq!(s.display_name_zh(), "Day+1 工具");
}
