use std::collections::HashMap;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_foundation::{MainThreadMarker, NSError, NSString};
use objc2_web_kit::{WKContentRuleList, WKContentRuleListStore};
use wry::{WebView, WebViewExtMacOS};

use crate::msg::{Msg, MsgSender};

pub struct Blocker {
    store: Option<Retained<WKContentRuleListStore>>,
    rule: Option<Retained<WKContentRuleList>>,
    enabled: bool,
    ready: bool,
}

impl Blocker {
    pub fn new() -> Self {
        Self {
            store: None,
            rule: None,
            enabled: true,
            ready: false,
        }
    }

    pub fn compile(&mut self, tx: MsgSender) {
        let mtm = MainThreadMarker::new().expect("content rules need the main thread");
        let Some(store) = (unsafe { WKContentRuleListStore::defaultStore(mtm) }) else {
            let _ = tx.send(Msg::BlockerFailed);
            return;
        };
        let callback = RcBlock::new(move |list: *mut WKContentRuleList, _error: *mut NSError| {
            if let Some(retained) = unsafe { Retained::retain(list) } {
                // Apple's WKContentRuleList is Sendable. Carry the retained pointer through
                // the event loop, then restore ownership on its main thread.
                let pointer = Retained::into_raw(retained) as usize;
                let _ = tx.send(Msg::BlockerReady(pointer));
            } else {
                let _ = tx.send(Msg::BlockerFailed);
            }
        });
        let identifier = NSString::from_str("browser-protection-v2");
        let rules = NSString::from_str(include_str!("blocker_rules.json"));
        unsafe {
            store.compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler(
                Some(&identifier),
                Some(&rules),
                Some(&callback),
            );
        }
        self.store = Some(store);
    }

    pub fn accept_compiled(&mut self, pointer: usize) {
        self.rule = unsafe { Retained::from_raw(pointer as *mut WKContentRuleList) };
        self.store = None;
        self.ready = true;
    }

    pub fn fail(&mut self) {
        self.store = None;
        self.ready = true;
    }

    pub fn ready(&self) -> bool {
        self.ready
    }

    pub fn available(&self) -> bool {
        self.rule.is_some()
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn active_rule(&self) -> Option<&WKContentRuleList> {
        if self.enabled {
            self.rule.as_deref()
        } else {
            None
        }
    }

    pub fn toggle(&mut self, contents: &HashMap<u32, WebView>) {
        let Some(rule) = self.rule.as_deref() else {
            return;
        };
        self.enabled = !self.enabled;
        for view in contents.values() {
            unsafe {
                if self.enabled {
                    view.manager().addContentRuleList(rule);
                } else {
                    view.manager().removeContentRuleList(rule);
                }
            }
        }
    }
}
