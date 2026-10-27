//! A code-entry buffer that wipes itself.
//!
//! Honest limits: the code still becomes a JS string on its way to HA and
//! travels over the websocket. Wiping here is defence in depth, not a
//! guarantee. What it buys: no code sitting in WASM memory after submit,
//! cancel, or teardown, and no code in debug output.

use core::fmt;

pub const MAX_CODE_LEN: usize = 16;

pub struct PinBuffer {
    bytes: Vec<u8>,
}

impl PinBuffer {
    pub fn new() -> Self {
        // Reserve once and never exceed it, so the Vec never reallocates and
        // leaves an unwiped copy behind in freed memory.
        Self {
            bytes: Vec::with_capacity(MAX_CODE_LEN),
        }
    }

    /// Append a digit. Returns false if it was rejected.
    pub fn push_digit(&mut self, c: char) -> bool {
        if self.bytes.len() >= MAX_CODE_LEN || !c.is_ascii_digit() {
            return false;
        }
        self.bytes.push(c as u8);
        true
    }

    pub fn pop(&mut self) {
        if let Some(last) = self.bytes.last_mut() {
            wipe_byte(last);
            let len = self.bytes.len() - 1;
            self.bytes.truncate(len);
        }
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn clear(&mut self) {
        for b in self.bytes.iter_mut() {
            wipe_byte(b);
        }
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        self.bytes.clear();
    }

    /// Hand the code out once and wipe the buffer.
    pub fn take(&mut self) -> String {
        let code: String = self.bytes.iter().map(|&b| char::from(b)).collect();
        self.clear();
        code
    }
}

impl Default for PinBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PinBuffer {
    fn drop(&mut self) {
        self.clear();
    }
}

impl fmt::Debug for PinBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PinBuffer(<{} digits redacted>)", self.bytes.len())
    }
}

/// Consume a string holding a code and zero its bytes before freeing them.
/// Use for the copies Rust makes on the way out to JS.
pub fn wipe_string(s: String) {
    let mut bytes = s.into_bytes();
    for b in bytes.iter_mut() {
        wipe_byte(b);
    }
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}

#[allow(unsafe_code)]
fn wipe_byte(b: &mut u8) {
    // SAFETY: `b` is a valid, aligned, exclusive reference. The volatile
    // write stops the compiler from optimising the wipe away.
    unsafe { core::ptr::write_volatile(b, 0) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_only_and_bounded() {
        let mut p = PinBuffer::new();
        assert!(!p.push_digit('a'));
        assert!(!p.push_digit(' '));
        for _ in 0..MAX_CODE_LEN {
            assert!(p.push_digit('7'));
        }
        assert!(!p.push_digit('7'));
        assert_eq!(p.len(), MAX_CODE_LEN);
    }

    #[test]
    fn take_returns_once_and_clears() {
        let mut p = PinBuffer::new();
        for c in "1234".chars() {
            p.push_digit(c);
        }
        p.pop();
        assert_eq!(p.take(), "123");
        assert!(p.is_empty());
        assert_eq!(p.take(), "");
    }

    #[test]
    fn debug_never_shows_digits() {
        let mut p = PinBuffer::new();
        p.push_digit('9');
        let s = format!("{p:?}");
        assert!(s.contains("redacted"));
        assert!(!s.contains('9'));
    }
}
