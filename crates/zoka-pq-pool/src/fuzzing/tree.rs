//! A value as postcard writes it, without its type.
//!
//! Postcard is not self-describing: bytes alone do not say where a vector
//! ends, so flipping bytes of an encoded proof almost always yields something
//! the decoder refuses before any real code runs. A [`Node`] keeps the shape a
//! typed value had when it was written. Editing the shape (drop an element,
//! repeat one, empty an option) and writing it back gives bytes that still
//! decode, into a value whose vectors have lengths nobody intended: the
//! inputs that find missing length checks.
use super::mutate::Rng;
use serde::{ser, Serialize};

/// The KoalaBear modulus: 32-bit values next to it are the ones a field
/// decoder has to refuse or reduce.
const FIELD: u128 = 0x7f00_0001;

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Bool(bool),
    /// `u8` and `i8`: one raw byte.
    Byte(u8),
    /// A varint, with the width of the type it came from.
    Unsigned {
        value: u128,
        bits: u8,
    },
    /// A zigzag varint.
    Signed {
        value: i128,
        bits: u8,
    },
    /// `f32` and `f64`: little-endian bytes, no prefix.
    Fixed(Vec<u8>),
    /// A string or byte string: length, then the bytes.
    Bytes(Vec<u8>),
    Option(Option<Box<Node>>),
    /// Length, then the elements.
    Seq(Vec<Node>),
    /// A tuple, struct or array: its fields and nothing else.
    Tuple(Vec<Node>),
    Variant(u32, Vec<Node>),
    Map(Vec<(Node, Node)>),
}

/// Where an edit landed and what it did, to tell accepted mutants apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    /// Child indices from the root (a map's entry `i` is `2i` and `2i + 1`).
    pub path: Vec<usize>,
    pub what: &'static str,
}

pub fn capture<T: Serialize + ?Sized>(value: &T) -> Result<Node, String> {
    value.serialize(Capture).map_err(|e| e.0)
}

impl Node {
    /// The bytes postcard writes for a value of this shape.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Self::Bool(v) => out.push(u8::from(*v)),
            Self::Byte(v) => out.push(*v),
            Self::Unsigned { value, .. } => varint(*value, out),
            Self::Signed { value, .. } => varint(((value << 1) ^ (value >> 127)) as u128, out),
            Self::Fixed(bytes) => out.extend_from_slice(bytes),
            Self::Bytes(bytes) => {
                varint(bytes.len() as u128, out);
                out.extend_from_slice(bytes);
            }
            Self::Option(None) => out.push(0),
            Self::Option(Some(inner)) => {
                out.push(1);
                inner.write(out);
            }
            Self::Seq(items) => {
                varint(items.len() as u128, out);
                items.iter().for_each(|item| item.write(out));
            }
            Self::Tuple(items) => items.iter().for_each(|item| item.write(out)),
            Self::Variant(index, items) => {
                varint(u128::from(*index), out);
                items.iter().for_each(|item| item.write(out));
            }
            Self::Map(entries) => {
                varint(entries.len() as u128, out);
                for (key, value) in entries {
                    key.write(out);
                    value.write(out);
                }
            }
        }
    }

    fn children(&self) -> usize {
        match self {
            Self::Option(Some(_)) => 1,
            Self::Seq(items) | Self::Tuple(items) | Self::Variant(_, items) => items.len(),
            Self::Map(entries) => entries.len() * 2,
            _ => 0,
        }
    }

    fn child_mut(&mut self, at: usize) -> Option<&mut Node> {
        match self {
            Self::Option(Some(inner)) if at == 0 => Some(inner),
            Self::Seq(items) | Self::Tuple(items) | Self::Variant(_, items) => items.get_mut(at),
            Self::Map(entries) => {
                entries
                    .get_mut(at / 2)
                    .map(|(key, value)| if at % 2 == 0 { key } else { value })
            }
            _ => None,
        }
    }

    pub fn at_mut(&mut self, path: &[usize]) -> Option<&mut Node> {
        match path.split_first() {
            None => Some(self),
            Some((first, rest)) => self.child_mut(*first)?.at_mut(rest),
        }
    }

    /// Where this tree and `other` differ, at most `limit` places: the path
    /// of each differing value, or of the node whose shape changed.
    pub fn differences(&self, other: &Node, limit: usize) -> Vec<Vec<usize>> {
        let mut found = Vec::new();
        self.compare(other, &mut Vec::new(), &mut found, limit);
        found
    }

    fn compare(
        &self,
        other: &Node,
        path: &mut Vec<usize>,
        found: &mut Vec<Vec<usize>>,
        limit: usize,
    ) {
        if found.len() >= limit {
            return;
        }
        let mut pairs = |a: &[Node], b: &[Node], path: &mut Vec<usize>| {
            for (at, (left, right)) in a.iter().zip(b).enumerate() {
                path.push(at);
                left.compare(right, path, found, limit);
                path.pop();
            }
        };
        match (self, other) {
            (Self::Option(Some(a)), Self::Option(Some(b))) => {
                pairs(std::slice::from_ref(a), std::slice::from_ref(b), path)
            }
            (Self::Seq(a), Self::Seq(b)) | (Self::Tuple(a), Self::Tuple(b))
                if a.len() == b.len() =>
            {
                pairs(a, b, path)
            }
            (Self::Variant(i, a), Self::Variant(j, b)) if i == j && a.len() == b.len() => {
                pairs(a, b, path)
            }
            (Self::Map(a), Self::Map(b)) if a.len() == b.len() => {
                for (at, ((ka, va), (kb, vb))) in a.iter().zip(b).enumerate() {
                    path.push(at * 2);
                    ka.compare(kb, path, found, limit);
                    path.pop();
                    path.push(at * 2 + 1);
                    va.compare(vb, path, found, limit);
                    path.pop();
                }
            }
            (a, b) => {
                if a != b && found.len() < limit {
                    found.push(path.clone());
                }
            }
        }
    }

    /// Something whose own shape can change, as opposed to its children.
    fn has_shape(&self) -> bool {
        match self {
            Self::Seq(items) => !items.is_empty(),
            Self::Bytes(bytes) => !bytes.is_empty(),
            Self::Map(entries) => !entries.is_empty(),
            Self::Option(Some(_)) | Self::Variant(..) => true,
            _ => false,
        }
    }

    /// A node to edit: every level of the tree is about as likely as any
    /// other, so the few outer vectors of a proof are reached as often as its
    /// thousands of field elements.
    fn pick(&mut self, rng: &mut Rng, path: &mut Vec<usize>) -> &mut Node {
        let children = self.children();
        if children == 0 || (self.has_shape() && rng.below(3) == 0) {
            return self;
        }
        let at = rng.below(children);
        path.push(at);
        self.child_mut(at)
            .expect("an index below the child count")
            .pick(rng, path)
    }

    /// One edit somewhere in the tree. `None` if the node drawn had nothing to
    /// change (an empty vector, say): draw again.
    pub fn mutate(&mut self, rng: &mut Rng) -> Option<Edit> {
        let mut path = Vec::new();
        let what = self.pick(rng, &mut path).edit(rng)?;
        Some(Edit { path, what })
    }

    fn edit(&mut self, rng: &mut Rng) -> Option<&'static str> {
        Some(match self {
            Self::Bool(v) => {
                *v = !*v;
                "flip"
            }
            Self::Byte(v) => {
                *v = match rng.below(4) {
                    0 => v.wrapping_add(1),
                    1 => v.wrapping_sub(1),
                    2 => *v ^ (1 << rng.below(8)),
                    _ => !*v,
                };
                "byte"
            }
            Self::Unsigned { value, bits } => {
                let max = if *bits >= 128 {
                    u128::MAX
                } else {
                    (1u128 << *bits) - 1
                };
                let before = *value;
                let (next, what) = match rng.below(10) {
                    0 => (0, "zero"),
                    1 => (1, "one"),
                    2 => (before.wrapping_add(1) & max, "plus one"),
                    3 => (before.wrapping_sub(1) & max, "minus one"),
                    4 => (before ^ (1 << rng.below(usize::from(*bits))), "bit"),
                    5 => (max, "maximum"),
                    // One more than the type holds: the decoder has to refuse.
                    6 if *bits < 128 => (max + 1, "overflow"),
                    7 => (FIELD, "field modulus"),
                    8 => (FIELD - 1, "field maximum"),
                    _ => (u128::from(rng.next()) & max, "random"),
                };
                if next == before {
                    return None;
                }
                *value = next;
                what
            }
            Self::Signed { value, .. } => {
                let before = *value;
                *value = match rng.below(4) {
                    0 => 0,
                    1 => before.wrapping_add(1),
                    2 => before.wrapping_neg(),
                    _ => before.wrapping_sub(1),
                };
                if *value == before {
                    return None;
                }
                "signed"
            }
            Self::Fixed(bytes) => {
                let at = rng.below(bytes.len().max(1));
                *bytes.get_mut(at)? ^= 1 << rng.below(8);
                "float bit"
            }
            Self::Bytes(bytes) => {
                let at = rng.below(bytes.len().max(1));
                match rng.below(5) {
                    0 if !bytes.is_empty() => {
                        bytes[at] ^= 1 << rng.below(8);
                        "bytes bit"
                    }
                    1 if !bytes.is_empty() => {
                        bytes.remove(at);
                        "bytes shorter"
                    }
                    2 if !bytes.is_empty() => {
                        bytes.truncate(at);
                        "bytes cut"
                    }
                    3 => {
                        bytes.push(rng.next() as u8);
                        "bytes longer"
                    }
                    _ if !bytes.is_empty() => {
                        bytes.clear();
                        "bytes empty"
                    }
                    _ => return None,
                }
            }
            Self::Option(inner @ Some(_)) => {
                *inner = None;
                "option empty"
            }
            Self::Option(None) => return None,
            Self::Seq(items) => {
                if items.is_empty() {
                    return None;
                }
                let at = rng.below(items.len());
                match rng.below(7) {
                    0 => {
                        items.remove(at);
                        "element removed"
                    }
                    1 => {
                        let copy = items[at].clone();
                        items.insert(at, copy);
                        "element repeated"
                    }
                    2 => {
                        items.truncate(at);
                        "sequence cut"
                    }
                    3 => {
                        items.clear();
                        "sequence empty"
                    }
                    4 => {
                        items.pop();
                        "last element removed"
                    }
                    5 if items.len() > 1 => {
                        let other = (at + 1 + rng.below(items.len() - 1)) % items.len();
                        if items[at] == items[other] {
                            return None;
                        }
                        items.swap(at, other);
                        "elements swapped"
                    }
                    _ if items.len() > 1 => {
                        let other = (at + 1 + rng.below(items.len() - 1)) % items.len();
                        if items[at] == items[other] {
                            return None;
                        }
                        items[at] = items[other].clone();
                        "element overwritten"
                    }
                    _ => return None,
                }
            }
            Self::Tuple(_) => return None,
            Self::Variant(index, _) => {
                *index ^= 1;
                "variant"
            }
            Self::Map(entries) => {
                if entries.is_empty() {
                    return None;
                }
                let at = rng.below(entries.len());
                if rng.below(2) == 0 {
                    entries.remove(at);
                    "entry removed"
                } else {
                    let copy = entries[at].clone();
                    entries.insert(at, copy);
                    "entry repeated"
                }
            }
        })
    }
}

fn varint(mut value: u128, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

#[derive(Debug)]
struct Unsupported(String);
impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Unsupported {}
impl ser::Error for Unsupported {
    fn custom<T: std::fmt::Display>(message: T) -> Self {
        Self(message.to_string())
    }
}

/// Writes nothing: records what postcard's serializer would be asked to write.
struct Capture;

fn unsigned(value: u128, bits: u8) -> Result<Node, Unsupported> {
    Ok(Node::Unsigned { value, bits })
}
fn signed(value: i128, bits: u8) -> Result<Node, Unsupported> {
    Ok(Node::Signed { value, bits })
}

impl ser::Serializer for Capture {
    type Ok = Node;
    type Error = Unsupported;
    type SerializeSeq = Items;
    type SerializeTuple = Items;
    type SerializeTupleStruct = Items;
    type SerializeTupleVariant = Items;
    type SerializeMap = Entries;
    type SerializeStruct = Items;
    type SerializeStructVariant = Items;

    // Postcard is a binary format: types choose their compact form.
    fn is_human_readable(&self) -> bool {
        false
    }
    fn serialize_bool(self, v: bool) -> Result<Node, Unsupported> {
        Ok(Node::Bool(v))
    }
    fn serialize_i8(self, v: i8) -> Result<Node, Unsupported> {
        Ok(Node::Byte(v as u8))
    }
    fn serialize_i16(self, v: i16) -> Result<Node, Unsupported> {
        signed(v.into(), 16)
    }
    fn serialize_i32(self, v: i32) -> Result<Node, Unsupported> {
        signed(v.into(), 32)
    }
    fn serialize_i64(self, v: i64) -> Result<Node, Unsupported> {
        signed(v.into(), 64)
    }
    fn serialize_i128(self, v: i128) -> Result<Node, Unsupported> {
        signed(v, 128)
    }
    fn serialize_u8(self, v: u8) -> Result<Node, Unsupported> {
        Ok(Node::Byte(v))
    }
    fn serialize_u16(self, v: u16) -> Result<Node, Unsupported> {
        unsigned(v.into(), 16)
    }
    fn serialize_u32(self, v: u32) -> Result<Node, Unsupported> {
        unsigned(v.into(), 32)
    }
    fn serialize_u64(self, v: u64) -> Result<Node, Unsupported> {
        unsigned(v.into(), 64)
    }
    fn serialize_u128(self, v: u128) -> Result<Node, Unsupported> {
        unsigned(v, 128)
    }
    fn serialize_f32(self, v: f32) -> Result<Node, Unsupported> {
        Ok(Node::Fixed(v.to_le_bytes().to_vec()))
    }
    fn serialize_f64(self, v: f64) -> Result<Node, Unsupported> {
        Ok(Node::Fixed(v.to_le_bytes().to_vec()))
    }
    fn serialize_char(self, v: char) -> Result<Node, Unsupported> {
        Ok(Node::Bytes(v.to_string().into_bytes()))
    }
    fn serialize_str(self, v: &str) -> Result<Node, Unsupported> {
        Ok(Node::Bytes(v.as_bytes().to_vec()))
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<Node, Unsupported> {
        Ok(Node::Bytes(v.to_vec()))
    }
    fn serialize_none(self) -> Result<Node, Unsupported> {
        Ok(Node::Option(None))
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Node, Unsupported> {
        Ok(Node::Option(Some(Box::new(value.serialize(Capture)?))))
    }
    fn serialize_unit(self) -> Result<Node, Unsupported> {
        Ok(Node::Tuple(Vec::new()))
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<Node, Unsupported> {
        Ok(Node::Tuple(Vec::new()))
    }
    fn serialize_unit_variant(
        self,
        _: &'static str,
        index: u32,
        _: &'static str,
    ) -> Result<Node, Unsupported> {
        Ok(Node::Variant(index, Vec::new()))
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<Node, Unsupported> {
        value.serialize(Capture)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        index: u32,
        _: &'static str,
        value: &T,
    ) -> Result<Node, Unsupported> {
        Ok(Node::Variant(index, vec![value.serialize(Capture)?]))
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Items, Unsupported> {
        Ok(Items::new(Kind::Seq))
    }
    fn serialize_tuple(self, _: usize) -> Result<Items, Unsupported> {
        Ok(Items::new(Kind::Tuple))
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Items, Unsupported> {
        Ok(Items::new(Kind::Tuple))
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        index: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Items, Unsupported> {
        Ok(Items::new(Kind::Variant(index)))
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Entries, Unsupported> {
        Ok(Entries {
            entries: Vec::new(),
            key: None,
        })
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Items, Unsupported> {
        Ok(Items::new(Kind::Tuple))
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        index: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Items, Unsupported> {
        Ok(Items::new(Kind::Variant(index)))
    }
}

enum Kind {
    Seq,
    Tuple,
    Variant(u32),
}
struct Items {
    kind: Kind,
    items: Vec<Node>,
}
impl Items {
    fn new(kind: Kind) -> Self {
        Self {
            kind,
            items: Vec::new(),
        }
    }
    fn push<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unsupported> {
        self.items.push(value.serialize(Capture)?);
        Ok(())
    }
    fn finish(self) -> Result<Node, Unsupported> {
        Ok(match self.kind {
            Kind::Seq => Node::Seq(self.items),
            Kind::Tuple => Node::Tuple(self.items),
            Kind::Variant(index) => Node::Variant(index, self.items),
        })
    }
}
impl ser::SerializeSeq for Items {
    type Ok = Node;
    type Error = Unsupported;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unsupported> {
        self.push(value)
    }
    fn end(self) -> Result<Node, Unsupported> {
        self.finish()
    }
}
impl ser::SerializeTuple for Items {
    type Ok = Node;
    type Error = Unsupported;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unsupported> {
        self.push(value)
    }
    fn end(self) -> Result<Node, Unsupported> {
        self.finish()
    }
}
impl ser::SerializeTupleStruct for Items {
    type Ok = Node;
    type Error = Unsupported;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unsupported> {
        self.push(value)
    }
    fn end(self) -> Result<Node, Unsupported> {
        self.finish()
    }
}
impl ser::SerializeTupleVariant for Items {
    type Ok = Node;
    type Error = Unsupported;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unsupported> {
        self.push(value)
    }
    fn end(self) -> Result<Node, Unsupported> {
        self.finish()
    }
}
impl ser::SerializeStruct for Items {
    type Ok = Node;
    type Error = Unsupported;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Unsupported> {
        self.push(value)
    }
    fn end(self) -> Result<Node, Unsupported> {
        self.finish()
    }
}
impl ser::SerializeStructVariant for Items {
    type Ok = Node;
    type Error = Unsupported;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Unsupported> {
        self.push(value)
    }
    fn end(self) -> Result<Node, Unsupported> {
        self.finish()
    }
}

struct Entries {
    entries: Vec<(Node, Node)>,
    key: Option<Node>,
}
impl ser::SerializeMap for Entries {
    type Ok = Node;
    type Error = Unsupported;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Unsupported> {
        self.key = Some(key.serialize(Capture)?);
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unsupported> {
        let key = self
            .key
            .take()
            .ok_or_else(|| Unsupported("a map value without its key".into()))?;
        self.entries.push((key, value.serialize(Capture)?));
        Ok(())
    }
    fn end(self) -> Result<Node, Unsupported> {
        Ok(Node::Map(self.entries))
    }
}
