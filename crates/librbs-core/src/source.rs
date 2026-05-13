use std::mem::ManuallyDrop;
use std::path::PathBuf;

use ruby_rbs::node::SignatureNode;

/// A buffer holding the file path and its content.
///
/// The Ruby counterpart (`RBS::Buffer`) also tracks per-line ranges and
/// provides `pos_to_loc`/`loc_to_pos` for character-offset based line/column
/// translation. We deliberately do not mirror that here: we construct
/// `RBS::Location` with raw position offsets and let the Ruby `RBS::Buffer`
/// do the line/column work, so the Rust side never needs to materialize line
/// offsets.
#[derive(Debug)]
pub struct Buffer {
    pub name: PathBuf,
    pub content: String,
}

impl Buffer {
    pub fn new(name: PathBuf, content: String) -> Self {
        Self { name, content }
    }
}

/// `ManagedParser` owns the parsed `SignatureNode` and the source content
/// it borrows from.
///
/// Soundness: `content` is heap-allocated through `Box<str>`; moving the
/// `ManagedParser` does not move the underlying bytes, so the `'static`
/// signature borrow remains valid for the lifetime of `self`. The signature
/// is dropped before the content via the explicit `Drop` impl.
pub struct ManagedParser {
    // Box<str> keeps the content at a stable heap address.
    content: Box<str>,
    signature: ManuallyDrop<SignatureNode<'static>>,
}

// Safety: each ManagedParser exclusively owns its parser; once parsing is
// complete we only read the AST, and the underlying C parser is not shared
// across threads. Sending one across thread boundaries is safe.
unsafe impl Send for ManagedParser {}
unsafe impl Sync for ManagedParser {}

impl ManagedParser {
    pub fn parse(content: String) -> Result<Self, String> {
        let content: Box<str> = content.into_boxed_str();
        // Safety: `content` is heap-stable; the `'static` borrow is
        // confined to `self` and dropped in `Drop`.
        let s: &'static str = unsafe { std::mem::transmute::<&str, &'static str>(&content) };
        let signature = ruby_rbs::node::parse(s)?;
        Ok(Self {
            content,
            signature: ManuallyDrop::new(signature),
        })
    }

    pub fn signature(&self) -> &SignatureNode<'_> {
        &self.signature
    }

    pub fn content(&self) -> &str {
        &self.content
    }
}

impl Drop for ManagedParser {
    fn drop(&mut self) {
        // Drop the signature (which frees the C parser) before content.
        unsafe { ManuallyDrop::drop(&mut self.signature) };
    }
}

impl std::fmt::Debug for ManagedParser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedParser")
            .field("content_len", &self.content.len())
            .finish()
    }
}

/// A parsed RBS source file, equivalent to `Source::RBS` in Ruby.
#[derive(Debug)]
pub struct Source {
    pub buffer: Buffer,
    pub parser: ManagedParser,
}

impl Source {
    pub fn new(path: PathBuf, content: String) -> Result<Self, String> {
        let parser = ManagedParser::parse(content.clone())?;
        let buffer = Buffer::new(path, content);
        Ok(Self { buffer, parser })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_trivial_rbs() {
        let p = ManagedParser::parse("class Foo end\n".to_string()).unwrap();
        assert!(p.signature().declarations().iter().count() >= 1);
    }
}
