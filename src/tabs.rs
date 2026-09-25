pub struct Tab {
    pub id: u32,
    pub title: String,
    pub url: String,
    pub is_active: bool,
    pub loading: bool,
    pub zoom: f64,
    /// No web view: never shown since launch, or unloaded to save memory.
    pub asleep: bool,
}

/// A closed tab that ⇧⌘T can bring back.
struct Closed {
    url: String,
    title: String,
    index: usize,
}

const CLOSED_LIMIT: usize = 25;

pub struct TabManager {
    pub tabs: Vec<Tab>,
    next_id: u32,
    pub active: u32,
    closed: Vec<Closed>,
    /// Background tabs opened from the active tab go after the previous ones.
    opened_from_active: usize,
}

impl TabManager {
    pub fn new() -> Self {
        Self {
            tabs: vec![],
            next_id: 1,
            active: 0,
            closed: vec![],
            opened_from_active: 0,
        }
    }

    pub fn has(&self, id: u32) -> bool {
        self.tabs.iter().any(|t| t.id == id)
    }

    pub fn index_of(&self, id: u32) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    fn push(&mut self, index: usize, url: &str, title: Option<&str>) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let title = title
            .filter(|t| !t.is_empty())
            .map_or_else(|| crate::url::display_title(url), str::to_string);
        self.tabs.insert(
            index.min(self.tabs.len()),
            Tab {
                id,
                title,
                url: url.into(),
                is_active: false,
                loading: false,
                zoom: 1.0,
                asleep: true,
            },
        );
        id
    }

    /// Opens a tab at the end and selects it.
    pub fn new_tab(&mut self, url: &str) -> u32 {
        let id = self.push(self.tabs.len(), url, None);
        self.switch(id);
        id
    }

    /// Opens a tab next to the active one, after tabs it already opened.
    pub fn open_child(&mut self, url: &str, select: bool) -> u32 {
        let index = self
            .index_of(self.active)
            .map_or(self.tabs.len(), |i| i + 1 + self.opened_from_active);
        let id = self.push(index, url, None);
        if select {
            self.switch(id);
        } else {
            self.opened_from_active += 1;
        }
        id
    }

    /// Adds a tab from a saved session without selecting it.
    pub fn restore(&mut self, url: &str, title: &str) -> u32 {
        self.push(self.tabs.len(), url, Some(title))
    }

    pub fn duplicate(&mut self, id: u32) -> Option<u32> {
        let index = self.index_of(id)?;
        let (url, title) = (self.tabs[index].url.clone(), self.tabs[index].title.clone());
        let new = self.push(index + 1, &url, Some(&title));
        self.switch(new);
        Some(new)
    }

    pub fn close_tab(&mut self, id: u32) {
        if let Some(i) = self.index_of(id) {
            let tab = self.tabs.remove(i);
            if crate::url::guard(&tab.url).is_some() && tab.url != crate::url::NEW_TAB {
                self.closed.push(Closed {
                    url: tab.url,
                    title: tab.title,
                    index: i,
                });
                if self.closed.len() > CLOSED_LIMIT {
                    self.closed.remove(0);
                }
            }
            if self.active == id && !self.tabs.is_empty() {
                let next = self.tabs[i.min(self.tabs.len() - 1)].id;
                self.switch(next);
            }
        }
        if self.tabs.is_empty() {
            self.active = 0;
        }
    }

    /// Brings back the most recently closed tab, selected, where it was.
    pub fn reopen(&mut self) -> Option<u32> {
        let closed = self.closed.pop()?;
        let id = self.push(closed.index, &closed.url, Some(&closed.title));
        self.switch(id);
        Some(id)
    }

    pub fn move_tab(&mut self, id: u32, index: usize) {
        if let Some(from) = self.index_of(id) {
            let tab = self.tabs.remove(from);
            self.tabs.insert(index.min(self.tabs.len()), tab);
        }
    }

    pub fn switch(&mut self, id: u32) {
        if self.has(id) {
            if self.active != id {
                self.opened_from_active = 0;
            }
            self.active = id;
            for t in &mut self.tabs {
                t.is_active = t.id == id;
            }
        }
    }

    pub fn get(&self, id: u32) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.get(self.active)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_switch_close() {
        let mut m = TabManager::new();
        let a = m.new_tab("https://example.com");
        let b = m.new_tab("https://example.org");
        assert!(m.get(b).unwrap().is_active);
        assert!(!m.get(a).unwrap().is_active);

        m.switch(999);
        assert_eq!(m.active, b);
        m.switch(a);
        assert_eq!(m.active, a);

        m.close_tab(b);
        assert_eq!(m.active, a);
        m.close_tab(a);
        assert!(m.tabs.is_empty());
        assert_eq!(m.active, 0);
    }

    #[test]
    fn children_open_in_order_after_parent() {
        let mut m = TabManager::new();
        let parent = m.new_tab("https://a.com");
        let last = m.new_tab("https://z.com");
        m.switch(parent);
        let one = m.open_child("https://1.com", false);
        let two = m.open_child("https://2.com", false);
        let order: Vec<u32> = m.tabs.iter().map(|t| t.id).collect();
        assert_eq!(order, vec![parent, one, two, last]);
        assert_eq!(m.active, parent);
    }

    #[test]
    fn reopen_restores_position() {
        let mut m = TabManager::new();
        m.new_tab("https://a.com");
        let b = m.new_tab("https://b.com");
        m.new_tab("https://c.com");
        m.close_tab(b);
        let back = m.reopen().unwrap();
        assert_eq!(m.index_of(back), Some(1));
        assert_eq!(m.get(back).unwrap().url, "https://b.com");
        assert_eq!(m.active, back);
        assert!(m.reopen().is_none());
    }

    #[test]
    fn move_and_duplicate() {
        let mut m = TabManager::new();
        let a = m.new_tab("https://a.com");
        let b = m.new_tab("https://b.com");
        m.move_tab(b, 0);
        assert_eq!(m.index_of(b), Some(0));
        let c = m.duplicate(a).unwrap();
        assert_eq!(m.index_of(c), Some(2));
        assert_eq!(m.get(c).unwrap().url, "https://a.com");
    }
}
