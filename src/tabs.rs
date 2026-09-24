pub struct Tab {
    pub id: u32,
    pub title: String,
    pub url: String,
    pub is_active: bool,
}

pub struct TabManager {
    pub tabs: Vec<Tab>,
    next_id: u32,
    pub active: u32,
}

impl TabManager {
    pub fn new(home: &str) -> Self {
        let mut m = Self {
            tabs: vec![],
            next_id: 1,
            active: 0,
        };
        m.new_tab(home);
        m
    }

    pub fn has(&self, id: u32) -> bool {
        self.tabs.iter().any(|t| t.id == id)
    }

    pub fn new_tab(&mut self, url: &str) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        for t in &mut self.tabs {
            t.is_active = false;
        }
        self.tabs.push(Tab {
            id,
            title: crate::url::display_title(url),
            url: url.into(),
            is_active: true,
        });
        self.active = id;
        id
    }

    pub fn close_tab(&mut self, id: u32) {
        if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
            self.tabs.remove(i);
            if self.active == id && !self.tabs.is_empty() {
                self.active = self.tabs[i.min(self.tabs.len() - 1)].id;
            }
        }
        if self.tabs.is_empty() {
            self.active = 0;
        } else if !self.has(self.active) {
            self.active = self.tabs.last().unwrap().id;
        }
        for t in &mut self.tabs {
            t.is_active = t.id == self.active;
        }
    }

    pub fn switch(&mut self, id: u32) {
        if self.has(id) {
            self.active = id;
            for t in &mut self.tabs {
                t.is_active = t.id == id;
            }
        }
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == self.active)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_switch_close() {
        let mut m = TabManager::new("https://example.com");
        let a = m.active;
        let b = m.new_tab("https://example.org");
        assert!(m.get_mut(b).unwrap().is_active);
        assert!(!m.get_mut(a).unwrap().is_active);
        assert!(m.tabs.iter().any(|t| t.id == a));

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
}
