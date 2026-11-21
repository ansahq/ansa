//! Local, bounded recents and favorites for emoji entries.
//!
//! This state is deliberately independent of general command history: it can
//! be cleared per collection, persists nothing by itself, and never exceeds
//! [`MAX_RECENTS`] or [`MAX_FAVORITES`].

/// Maximum stored recent entries.
pub const MAX_RECENTS: usize = 64;

/// Maximum stored favorite entries.
pub const MAX_FAVORITES: usize = 64;

/// Recents and favorites keyed by stable entry identities.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Collections {
    recents: Vec<String>,
    favorites: Vec<String>,
}

impl Collections {
    /// Creates an empty collection set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a use, moving the identity to the front and retaining at most
    /// [`MAX_RECENTS`] entries. Empty identities are ignored.
    pub fn record_recent(&mut self, id: impl Into<String>) {
        let id = id.into();
        if id.is_empty() {
            return;
        }
        self.recents.retain(|recent| recent != &id);
        self.recents.insert(0, id);
        self.recents.truncate(MAX_RECENTS);
    }

    /// Adds or removes a favorite. Returns `true` when the identity is now a
    /// favorite. Empty identities are ignored. New favorites are placed first
    /// and bounded by [`MAX_FAVORITES`].
    pub fn toggle_favorite(&mut self, id: &str) -> bool {
        if id.is_empty() {
            return false;
        }
        if let Some(index) = self.favorites.iter().position(|favorite| favorite == id) {
            self.favorites.remove(index);
            false
        } else {
            self.favorites.retain(|favorite| favorite != id);
            self.favorites.insert(0, id.to_owned());
            self.favorites.truncate(MAX_FAVORITES);
            true
        }
    }

    /// Whether the identity is a current favorite.
    pub fn is_favorite(&self, id: &str) -> bool {
        self.favorites.iter().any(|favorite| favorite == id)
    }

    /// Recents, most recent first.
    pub fn recents(&self) -> &[String] {
        &self.recents
    }

    /// Favorites, most recently added first.
    pub fn favorites(&self) -> &[String] {
        &self.favorites
    }

    /// Clears recents without touching favorites.
    pub fn clear_recents(&mut self) {
        self.recents.clear();
    }

    /// Clears favorites without touching recents.
    pub fn clear_favorites(&mut self) {
        self.favorites.clear();
    }

    /// Clears both collections.
    pub fn clear_all(&mut self) {
        self.recents.clear();
        self.favorites.clear();
    }
}
