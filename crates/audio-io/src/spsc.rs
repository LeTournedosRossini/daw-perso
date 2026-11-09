//! File lock-free à un producteur et un consommateur, pour des mots de 32 bits.
//!
//! Sert à sortir des mesures du callback audio vers un autre thread sans verrou ni allocation
//! (la mémoire est allouée à la création). Entièrement en code sûr : chaque case est un atomique.
//! Contrat : un seul thread appelle `push`, un seul appelle `pop`.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

pub struct Spsc {
    buf: Box<[AtomicU32]>,
    /// Nombre total d'éléments écrits (croît sans fin, réduit modulo la capacité à l'accès).
    head: AtomicUsize,
    /// Nombre total d'éléments lus.
    tail: AtomicUsize,
}

impl Spsc {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Spsc {
            buf: (0..capacity).map(|_| AtomicU32::new(0)).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Renvoie `false` si la file est pleine (la valeur est alors perdue).
    pub fn push(&self, value: u32) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= self.buf.len() {
            return false;
        }
        self.buf[head % self.buf.len()].store(value, Ordering::Relaxed);
        self.head.store(head.wrapping_add(1), Ordering::Release);
        true
    }

    pub fn pop(&self) -> Option<u32> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail == head {
            return None;
        }
        let value = self.buf[tail % self.buf.len()].load(Ordering::Relaxed);
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn fifo_order_and_full() {
        let q = Spsc::new(3);
        assert!(q.push(1) && q.push(2) && q.push(3));
        assert!(!q.push(4), "pleine");
        assert_eq!(q.pop(), Some(1));
        assert!(q.push(4));
        assert_eq!((q.pop(), q.pop(), q.pop(), q.pop()), (Some(2), Some(3), Some(4), None));
    }

    #[test]
    fn two_threads() {
        let q = Arc::new(Spsc::new(16));
        let p = Arc::clone(&q);
        let t = std::thread::spawn(move || {
            let mut i = 0u32;
            while i < 10_000 {
                if p.push(i) {
                    i += 1;
                }
            }
        });
        let mut expected = 0u32;
        while expected < 10_000 {
            if let Some(v) = q.pop() {
                assert_eq!(v, expected);
                expected += 1;
            }
        }
        t.join().unwrap();
    }
}
