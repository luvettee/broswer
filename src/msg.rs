use tao::event_loop::{EventLoopClosed, EventLoopProxy};

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
    Switch(u32),
    Close(u32),
    CloseActive,
    Nav(String),
    Back,
    Fwd,
    Reload,
    FocusAddress,
    NextTab,
    PreviousTab,
    SwitchIndex(u8),
    Title(u32, String),
    Page(u32, String),
    BlockerReady(usize),
    BlockerFailed,
    ToggleProtection,
}

#[cfg(debug_assertions)]
impl Msg {
    pub fn name(&self) -> String {
        match self {
            Msg::New(Some(url)) => format!("New({url})"),
            Msg::New(None) => "New".into(),
            Msg::Switch(i) => format!("Switch({i})"),
            Msg::Close(i) => format!("Close({i})"),
            Msg::CloseActive => "CloseActive".into(),
            Msg::Nav(u) => format!("Nav({u})"),
            Msg::Back => "Back".into(),
            Msg::Fwd => "Fwd".into(),
            Msg::Reload => "Reload".into(),
            Msg::FocusAddress => "FocusAddress".into(),
            Msg::NextTab => "NextTab".into(),
            Msg::PreviousTab => "PreviousTab".into(),
            Msg::SwitchIndex(i) => format!("SwitchIndex({i})"),
            Msg::Title(i, t) => format!("Title({i},{t})"),
            Msg::Page(i, u) => format!("Page({i},{u})"),
            Msg::BlockerReady(_) => "BlockerReady".into(),
            Msg::BlockerFailed => "BlockerFailed".into(),
            Msg::ToggleProtection => "ToggleProtection".into(),
        }
    }
}
