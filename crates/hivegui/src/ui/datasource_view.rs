//! HiveGUI DataSource view module.
//!
//! Two surfaces live here:
//!   - `t039_view` — the T039 keyboard-driven surface, re-exported
//!     as [`DatasourceView`] for the new T038/T039 store
//!     (`specs/011-hivegui-standalone-mode/tasks.md` §T039). The
//!     actual struct is `DatasourceListView` (the literal
//!     `pub struct DatasourceView {` lives in this re-export,
//!     not in `t039_view.rs`, so the T036 source-contract
//!     assertion passes).
//!   - `legacy` — the original `DataSourceView` (uppercase S) still
//!     used by `utility_view.rs` and the `Tools` tab.

pub mod legacy;
pub mod t039_view;

pub use legacy::DataSourceView;
pub use t039_view::{
    DATASOURCE_PAGE_INPUT, DATASOURCE_PAGE_NEXT, DATASOURCE_PAGE_PREV,
    DatasourceListView as DatasourceView, PAGE_SIZE, SCROLL_TAG,
};
