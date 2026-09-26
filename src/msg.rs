use tao::event_loop::{EventLoopClosed, EventLoopProxy};

use crate::filters::Converted;

#[derive(Clone)]
pub struct MsgSender(EventLoopProxy<Msg>);

impl MsgSender {
    pub fn new(proxy: EventLoopProxy<Msg>) -> Self {
        Self(proxy)
    }

    pub fn send(&self, msg: Msg) -> Result<(), EventLoopClosed<Msg>> {
        self.0.send_event(msg)
    }
}

pub enum Msg {
    New(Option<String>),
    /// A link the user asked to open in another tab; `select` brings it forward.
    OpenLink {
        url: String,
        select: bool,
    },
    Switch(u32),
    Close(u32),
    CloseActive,
    CloseOthers(u32),
    CloseBelow(u32),
    Duplicate(u32),
    ReloadTab(u32),
    CopyAddress(u32),
    Move(u32, usize),
    Reopen,
    Nav(String),
    Back,
    Fwd,
    Reload,
    FocusAddress,
    /// Escape in the address field: restore the page address and focus the page.
    CancelAddress,
    NextTab,
    PreviousTab,
    SwitchIndex(u8),
    Stop,
    /// Something observable changed on a tab's web view (address, title, loading...).
    PageChanged(u32),
    Hover(u32, String),
    ShowFind,
    HideFind,
    Find {
        query: String,
        backwards: bool,
        /// Typing in the field; search from the current match, not after it.
        typing: bool,
    },
    FindNext(bool),
    FindResult(bool),
    /// -1 zoom out, 0 actual size, 1 zoom in.
    Zoom(i8),
    Print,
    /// The address field's text changed while editing.
    AddressTyped(String),
    AddressEnded,
    /// Arrow keys in the address field: move through suggestions.
    SuggestMove(i8),
    PickSuggestion(usize),
    /// Opens an address from a menu (bookmark or history) in the current tab.
    OpenUrl(String),
    ToggleBookmark,
    EditBookmarks,
    ClearHistory,
    /// A page named its icon.
    IconFound(u32, String),
    IconLoaded(String, Option<Vec<u8>>),
    ToggleSidebar,
    /// Show or hide the sidebar over the page while it is collapsed.
    SidebarPeek(bool),
    /// Result of checking a background tab before unloading it.
    UnloadCheck {
        id: u32,
        playing: bool,
    },
    /// macOS is short of memory; free everything that can be rebuilt.
    MemoryPressure {
        critical: bool,
    },
    FiltersConverted {
        generation: u64,
        converted: Converted,
        builtin_only: bool,
    },
    RuleListReady {
        generation: u64,
        index: usize,
        pointer: Option<usize>,
        error: Option<String>,
    },
    FiltersDownloaded {
        changed: bool,
        failed: Vec<String>,
    },
    ToggleProtection,
    ToggleSiteProtection,
    ToggleFilterList(usize),
    ToggleGenericHiding,
    ToggleLockdown,
    SetIdleMinutes(Option<u8>),
    UpdateFilters,
    EditCustomFilters,
}

#[cfg(debug_assertions)]
impl Msg {
    pub fn name(&self) -> String {
        match self {
            Msg::New(Some(url)) => format!("New({url})"),
            Msg::New(None) => "New".into(),
            Msg::OpenLink { url, select } => format!("OpenLink({url},{select})"),
            Msg::Switch(i) => format!("Switch({i})"),
            Msg::Close(i) => format!("Close({i})"),
            Msg::CloseActive => "CloseActive".into(),
            Msg::CloseOthers(i) => format!("CloseOthers({i})"),
            Msg::CloseBelow(i) => format!("CloseBelow({i})"),
            Msg::Duplicate(i) => format!("Duplicate({i})"),
            Msg::ReloadTab(i) => format!("ReloadTab({i})"),
            Msg::CopyAddress(i) => format!("CopyAddress({i})"),
            Msg::Move(i, to) => format!("Move({i},{to})"),
            Msg::Reopen => "Reopen".into(),
            Msg::Nav(u) => format!("Nav({u})"),
            Msg::Back => "Back".into(),
            Msg::Fwd => "Fwd".into(),
            Msg::Reload => "Reload".into(),
            Msg::FocusAddress => "FocusAddress".into(),
            Msg::CancelAddress => "CancelAddress".into(),
            Msg::NextTab => "NextTab".into(),
            Msg::PreviousTab => "PreviousTab".into(),
            Msg::SwitchIndex(i) => format!("SwitchIndex({i})"),
            Msg::Stop => "Stop".into(),
            Msg::PageChanged(i) => format!("PageChanged({i})"),
            Msg::Hover(i, u) => format!("Hover({i},{u})"),
            Msg::ShowFind => "ShowFind".into(),
            Msg::HideFind => "HideFind".into(),
            Msg::Find { query, .. } => format!("Find({query})"),
            Msg::FindNext(back) => format!("FindNext({back})"),
            Msg::FindResult(found) => format!("FindResult({found})"),
            Msg::Zoom(step) => format!("Zoom({step})"),
            Msg::Print => "Print".into(),
            Msg::AddressTyped(t) => format!("AddressTyped({t})"),
            Msg::AddressEnded => "AddressEnded".into(),
            Msg::SuggestMove(d) => format!("SuggestMove({d})"),
            Msg::PickSuggestion(i) => format!("PickSuggestion({i})"),
            Msg::OpenUrl(u) => format!("OpenUrl({u})"),
            Msg::ToggleBookmark => "ToggleBookmark".into(),
            Msg::EditBookmarks => "EditBookmarks".into(),
            Msg::ClearHistory => "ClearHistory".into(),
            Msg::IconFound(i, u) => format!("IconFound({i},{u})"),
            Msg::IconLoaded(site, bytes) => {
                format!("IconLoaded({site},{:?})", bytes.as_ref().map(Vec::len))
            }
            Msg::ToggleSidebar => "ToggleSidebar".into(),
            Msg::SidebarPeek(show) => format!("SidebarPeek({show})"),
            Msg::UnloadCheck { id, playing } => format!("UnloadCheck({id},{playing})"),
            Msg::MemoryPressure { critical } => format!("MemoryPressure({critical})"),
            Msg::FiltersConverted { generation, .. } => format!("FiltersConverted({generation})"),
            Msg::RuleListReady {
                generation, index, ..
            } => format!("RuleListReady({generation},{index})"),
            Msg::FiltersDownloaded { changed, .. } => format!("FiltersDownloaded({changed})"),
            Msg::ToggleProtection => "ToggleProtection".into(),
            Msg::ToggleSiteProtection => "ToggleSiteProtection".into(),
            Msg::ToggleFilterList(i) => format!("ToggleFilterList({i})"),
            Msg::ToggleGenericHiding => "ToggleGenericHiding".into(),
            Msg::ToggleLockdown => "ToggleLockdown".into(),
            Msg::SetIdleMinutes(minutes) => format!("SetIdleMinutes({minutes:?})"),
            Msg::UpdateFilters => "UpdateFilters".into(),
            Msg::EditCustomFilters => "EditCustomFilters".into(),
        }
    }
}
