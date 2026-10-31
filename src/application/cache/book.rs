//! Thread-safe order book cache with optional update notifications.
//!
//! Provides concurrent read/write access to order book snapshots, with optional
//! broadcast notifications for update subscribers (e.g., cluster detection).

use parking_lot::RwLock;
use std::collections::HashMap;
use tokio::sync::broadcast;

use crate::domain::{book::Book, id::TokenId};

/// Notification sent when an order book is updated.
///
/// Subscribers receive this message via broadcast channel to trigger
/// downstream processing (e.g., strategy detection, cluster marking).
#[derive(Debug, Clone)]
pub struct BookUpdate {
    /// Identifier of the token whose order book was updated.
    pub token_id: TokenId,
}

/// Thread-safe cache of order books with optional broadcast notifications.
///
/// Stores the latest order book snapshot for each token. All read and write
/// operations are atomic and safe for concurrent access from multiple tasks.
///
/// # Notifications
///
/// When created with [`BookCache::with_notifications`], the cache broadcasts
/// update events that downstream services can subscribe to for reactive processing.
pub struct BookCache {
    /// Order books indexed by token ID.
    books: RwLock<HashMap<TokenId, Book>>,
    /// Broadcast sender for update notifications.
    /// Wrapped in `Option` to allow construction without notifications.
    tx: Option<broadcast::Sender<BookUpdate>>,
}

impl BookCache {
    /// Create a new cache without update notifications.
    #[must_use]
    pub fn new() -> Self {
        Self {
            books: RwLock::new(HashMap::new()),
            tx: None,
        }
    }

    /// Create a new cache with broadcast notifications.
    ///
    /// Returns the cache and an initial receiver for subscribing to updates.
    /// Additional receivers can be created via [`subscribe`](Self::subscribe).
    ///
    /// # Arguments
    ///
    /// * `capacity` - Maximum number of pending messages in the broadcast channel.
    ///   Slow receivers that fall behind will miss messages.
    #[must_use]
    pub fn with_notifications(capacity: usize) -> (Self, broadcast::Receiver<BookUpdate>) {
        let (tx, rx) = broadcast::channel(capacity);
        let cache = Self {
            books: RwLock::new(HashMap::new()),
            tx: Some(tx),
        };
        (cache, rx)
    }

    /// Subscribe to order book update notifications.
    ///
    /// Returns `None` if the cache was created without notifications.
    #[must_use]
    pub fn subscribe(&self) -> Option<broadcast::Receiver<BookUpdate>> {
        self.tx.as_ref().map(|tx| tx.subscribe())
    }

    /// Update an order book in the cache and notify subscribers.
    ///
    /// Replaces any existing book for the token. If notifications are enabled,
    /// broadcasts a [`BookUpdate`] message to all subscribers.
    pub fn update(&self, book: Book) {
        let token_id = book.token_id().clone();
        {
            let mut books = self.books.write();
            if books.get(&token_id).is_some_and(|old| matches!((old.source_timestamp(), book.source_timestamp()), (Some(a), Some(b)) if b < a)) { return; }
            books.insert(token_id.clone(), book);
        }

        // Notify subscribers (ignore send errors when no receivers exist)
        if let Some(ref tx) = self.tx {
            let _ = tx.send(BookUpdate { token_id });
        }
    }

    /// Retrieve a snapshot of an order book.
    ///
    /// Returns `None` if no book exists for the given token.
    #[must_use]
    pub fn get(&self, token_id: &TokenId) -> Option<Book> {
        self.books
            .read()
            .get(token_id)
            .filter(|book| book.is_fresh())
            .cloned()
    }

    /// Retrieve snapshots of two order books atomically.
    ///
    /// Holds a single read lock while fetching both books, ensuring
    /// a consistent view when books are updated concurrently.
    #[must_use]
    pub fn get_pair(&self, token_a: &TokenId, token_b: &TokenId) -> (Option<Book>, Option<Book>) {
        let books = self.books.read();
        (
            books.get(token_a).filter(|book| book.is_fresh()).cloned(),
            books.get(token_b).filter(|book| book.is_fresh()).cloned(),
        )
    }

    /// Retrieve snapshots of multiple order books atomically.
    ///
    /// Holds a single read lock while fetching all books, ensuring
    /// a consistent view across all requested tokens.
    #[must_use]
    pub fn get_many(&self, token_ids: &[TokenId]) -> Vec<Option<Book>> {
        let books = self.books.read();
        token_ids
            .iter()
            .map(|id| books.get(id).filter(|book| book.is_fresh()).cloned())
            .collect()
    }

    /// Invalidate all prices after a transport discontinuity.
    pub fn clear(&self) {
        self.books.write().clear();
    }

    /// Return the number of order books in the cache.
    #[must_use]
    pub fn len(&self) -> usize {
        self.books.read().len()
    }

    /// Return true if the cache contains no order books.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for BookCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::book::PriceLevel;
    use rust_decimal_macros::dec;

    #[test]
    fn test_update_and_get() {
        let cache = BookCache::new();
        let token_id = TokenId::from("test-token");

        let book = Book::with_levels(
            token_id.clone(),
            vec![PriceLevel::new(dec!(0.45), dec!(100))],
            vec![PriceLevel::new(dec!(0.50), dec!(100))],
        );

        cache.update(book);

        let retrieved = cache.get(&token_id);
        assert!(retrieved.is_some());

        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.best_bid().unwrap().price(), dec!(0.45));
        assert_eq!(retrieved.best_ask().unwrap().price(), dec!(0.50));
    }

    #[test]
    fn test_get_pair() {
        let cache = BookCache::new();
        let token_a = TokenId::from("token-a");
        let token_b = TokenId::from("token-b");

        let book_a = Book::with_levels(
            token_a.clone(),
            vec![],
            vec![PriceLevel::new(dec!(0.40), dec!(50))],
        );
        let book_b = Book::with_levels(
            token_b.clone(),
            vec![],
            vec![PriceLevel::new(dec!(0.55), dec!(75))],
        );

        cache.update(book_a);
        cache.update(book_b);

        let (a, b) = cache.get_pair(&token_a, &token_b);
        assert!(a.is_some());
        assert!(b.is_some());

        assert_eq!(a.unwrap().best_ask().unwrap().price(), dec!(0.40));
        assert_eq!(b.unwrap().best_ask().unwrap().price(), dec!(0.55));
    }

    #[test]
    fn test_get_many() {
        let cache = BookCache::new();
        let tokens: Vec<TokenId> = (0..3)
            .map(|i| TokenId::from(format!("token-{i}")))
            .collect();

        for (i, token) in tokens.iter().enumerate() {
            let price = rust_decimal::Decimal::from(i as u32 + 1) / rust_decimal::Decimal::from(10);
            let book = Book::with_levels(
                token.clone(),
                vec![],
                vec![PriceLevel::new(price, dec!(100))],
            );
            cache.update(book);
        }

        let results = cache.get_many(&tokens);
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| r.is_some()));
    }

    #[tokio::test]
    async fn test_with_notifications() {
        let (cache, mut rx) = BookCache::with_notifications(16);
        let token_id = TokenId::from("test-token");

        let book = Book::with_levels(
            token_id.clone(),
            vec![PriceLevel::new(dec!(0.45), dec!(100))],
            vec![],
        );

        cache.update(book);

        let update = rx.recv().await.unwrap();
        assert_eq!(update.token_id.as_str(), "test-token");
    }

    #[test]
    fn test_subscribe() {
        let (cache, _rx) = BookCache::with_notifications(16);
        let rx2 = cache.subscribe();
        assert!(rx2.is_some());

        let cache_no_notify = BookCache::new();
        assert!(cache_no_notify.subscribe().is_none());
    }
}
