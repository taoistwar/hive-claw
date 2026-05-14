use std::any::Any;

use crate::ui::strings_zh;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolSeriesKind {
    DayPlusOne,
    HourPlusOne,
}

impl ToolSeriesKind {
    pub fn display_name_zh(self) -> &'static str {
        match self {
            ToolSeriesKind::DayPlusOne => strings_zh::DAY_PLUS_ONE_LABEL,
            ToolSeriesKind::HourPlusOne => strings_zh::HOUR_PLUS_ONE_LABEL,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HelperToolId(pub String);

pub struct ToolSeries {
    pub kind: ToolSeriesKind,
    pub tools: Vec<HelperTool>,
}

impl ToolSeries {
    /// Construct the v1 series for `kind`. Both series ship with an empty
    /// tool list (FR-013, FR-014).
    pub fn for_kind(kind: ToolSeriesKind) -> Self {
        ToolSeries {
            kind,
            tools: Vec::new(),
        }
    }

    pub fn display_name_zh(&self) -> &'static str {
        self.kind.display_name_zh()
    }
}

pub struct HelperTool {
    pub id: HelperToolId,
    pub series: ToolSeriesKind,
    pub display_name_zh: String,
    pub description_zh: String,
    pub surface: Box<dyn HelperToolSurface>,
}

/// The session-scoped working surface a `HelperTool` renders inside the
/// HiveGUI shell. v1 defines the trait but ships zero implementors
/// (invariant H3). Implementors must own their own state so the HiveGUI
/// shell can keep them alive while the engineer navigates away and back.
pub trait HelperToolSurface: Any + Send + 'static {
    fn name(&self) -> &str;
}
