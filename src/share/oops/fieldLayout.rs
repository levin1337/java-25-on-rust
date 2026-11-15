/*
 * Port of OpenJDK 25 HotSpot FieldLayoutBuilder (share/classfile/fieldLayoutBuilder.hpp, fieldLayoutBuilder.cpp)
 *
 * Computes memory layouts and byte offsets for non-static and static fields:
 * - Proper header alignment based on ObjLayout (compact, compressed, or uncompressed)
 * - Best-fit placement into holes inherited from the superclass layout
 *   (this is what lets the first 4-byte field of a class occupy the 4-byte
 *   "klass gap" left by the compressed Klass pointer, and lets subclasses
 *   fill padding holes left by the superclass)
 * - Primitive fields allocated before oop fields (reversed when the
 *   superclass layout ends with a reference), each group sorted by size
 *   descending — HotSpot `FieldLayoutBuilder::compute_regular_layout`
 * - Natural field alignment (8-byte, 4-byte, 2-byte, 1-byte)
 * - Object padding to heap word (8 bytes) boundary
 */

use super::fieldInfo::FieldInfo;
use super::objLayout::ObjLayout;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutBlockKind {
    Empty,
    Reserved,
    Padding,
    Regular,
    Inherited,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutRawBlock {
    pub kind: LayoutBlockKind,
    pub offset: usize,
    pub size: usize,
    pub alignment: usize,
    pub field_index: Option<usize>,
    /// Whether the field occupying this block is a reference type.
    /// Carried through `Inherited` blocks so subclasses can reproduce
    /// HotSpot's `_super_ends_with_oop` field-ordering decision.
    pub is_reference: bool,
}

impl LayoutRawBlock {
    fn occupied(&self) -> bool {
        matches!(self.kind, LayoutBlockKind::Regular | LayoutBlockKind::Inherited)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldLayoutResult {
    pub instance_size: usize,
    pub nonstatic_field_size: usize,
    pub blocks: Vec<LayoutRawBlock>,
    /// HotSpot `_super_ends_with_oop` analogue for the *next* subclass:
    /// `true` when the highest-offset field block of this layout is a
    /// reference.  Decides whether a subclass allocates its oop fields
    /// before its primitive fields.
    pub ends_with_reference: bool,
}

pub struct FieldLayout;

/// Align `value` up to `alignment` (power of two), saturating at
/// `usize::MAX` so unbounded tail intervals stay representable.
#[inline]
fn align_up_saturating(value: usize, alignment: usize) -> usize {
    debug_assert!(alignment > 0 && (alignment & (alignment - 1)) == 0);
    value.checked_add(alignment - 1).map_or(usize::MAX, |v| v & !(alignment - 1))
}

impl FieldLayout {
    /// Computes byte offsets for instance (non-static) fields.
    ///
    /// `super_instance_size`-only variant kept for compatibility: the
    /// superclass layout is approximated by a single inherited block, so
    /// no superclass holes can be filled.  Prefer
    /// [`FieldLayout::compute_instance_layout_inheriting`] which receives
    /// the real superclass [`FieldLayoutResult`] and reproduces HotSpot's
    /// hole-filling behaviour.
    pub fn compute_instance_layout(
        layout: &ObjLayout,
        super_instance_size: usize,
        fields: &mut [FieldInfo],
    ) -> FieldLayoutResult {
        let super_layout = if super_instance_size > 0 {
            let header = layout.header_size_in_bytes();
            Some(FieldLayoutResult {
                instance_size: super_instance_size,
                nonstatic_field_size: 0,
                blocks: vec![
                    LayoutRawBlock {
                        kind: LayoutBlockKind::Reserved,
                        offset: 0,
                        size: header,
                        alignment: ObjLayout::HEAP_WORD_SIZE,
                        field_index: None,
                        is_reference: false,
                    },
                    LayoutRawBlock {
                        kind: LayoutBlockKind::Inherited,
                        offset: header,
                        size: super_instance_size.saturating_sub(header),
                        alignment: ObjLayout::HEAP_WORD_SIZE,
                        field_index: None,
                        is_reference: false,
                    },
                ],
                ends_with_reference: false,
            })
        } else {
            None
        };
        Self::compute_instance_layout_inheriting(layout, super_layout.as_ref(), fields)
    }

    /// HotSpot-faithful instance layout: fields are placed by best-fit
    /// into the free intervals left by the header and the superclass
    /// field blocks.  `super_layout` must be the [`FieldLayoutResult`]
    /// produced for the direct superclass (`None` for `java.lang.Object`).
    pub fn compute_instance_layout_inheriting(
        layout: &ObjLayout,
        super_layout: Option<&FieldLayoutResult>,
        fields: &mut [FieldInfo],
    ) -> FieldLayoutResult {
        let header_size = layout.header_size_in_bytes();

        // Occupied byte ranges: the object header plus every field block
        // inherited from the superclass.  Empty/Padding blocks of the
        // superclass stay free, mirroring HotSpot's reconstruct_layout +
        // fill_holes (non-contended classes).
        let mut occupied: Vec<(usize, usize)> = vec![(0, header_size)];
        if let Some(sup) = super_layout {
            for b in &sup.blocks {
                if b.occupied() {
                    occupied.push((b.offset, b.offset.saturating_add(b.size)));
                }
            }
        }
        occupied.sort_unstable();

        // Free intervals as (start, end); `usize::MAX` end marks the
        // unbounded tail after the last occupied byte.
        let mut free: Vec<(usize, usize)> = Vec::new();
        let mut cursor = header_size;
        let mut inherited_end = header_size;
        for &(start, end) in &occupied {
            if start > cursor {
                free.push((cursor, start));
            }
            cursor = cursor.max(end);
            inherited_end = inherited_end.max(end);
        }
        free.push((cursor, usize::MAX));

        // Split non-static fields into primitive and oop groups; each is
        // sorted by size descending, ties by declaration order —
        // HotSpot `FieldGroup::sort_by_size` + `compute_regular_layout`.
        let mut primitives: Vec<usize> = Vec::new();
        let mut oops: Vec<usize> = Vec::new();
        for (index, field) in fields.iter().enumerate() {
            if field.is_static() {
                continue;
            }
            if field.is_reference() {
                oops.push(index);
            } else {
                primitives.push(index);
            }
        }
        let by_size_desc = |a: &usize, b: &usize| {
            fields[*b]
                .size_in_bytes()
                .cmp(&fields[*a].size_in_bytes())
                .then_with(|| a.cmp(b))
        };
        primitives.sort_by(by_size_desc);
        oops.sort_by(by_size_desc);

        // When the superclass layout ends with a reference, HotSpot
        // allocates oop fields first so the trailing oop-map entry folds
        // into the superclass's; otherwise primitives go first.
        let order: Vec<usize> = if super_layout.is_some_and(|s| s.ends_with_reference) {
            oops.into_iter().chain(primitives).collect()
        } else {
            primitives.into_iter().chain(oops).collect()
        };

        let mut placed: Vec<LayoutRawBlock> = Vec::new();
        let mut content_end = inherited_end;

        for field_index in order {
            let (align, size, is_ref) = {
                let f = &fields[field_index];
                (f.alignment().max(1), f.size_in_bytes(), f.is_reference())
            };

            // Best-fit: the smallest free interval that can hold the
            // field at its required alignment (`FieldLayout::add`).
            let mut best: Option<usize> = None;
            for (i, &(start, end)) in free.iter().enumerate() {
                let aligned = align_up_saturating(start, align);
                let fits = aligned
                    .checked_add(size)
                    .is_some_and(|field_end| field_end <= end);
                if fits
                    && best.is_none_or(|b| {
                        end.saturating_sub(start) < free[b].1.saturating_sub(free[b].0)
                    })
                {
                    best = Some(i);
                }
            }
            // The unbounded tail interval always satisfies `fits`.
            let slot = best.expect("unbounded tail interval must fit any field");
            let (start, end) = free[slot];
            let field_offset = align_up_saturating(start, align);
            let field_end = field_offset + size;

            fields[field_index].set_offset(field_offset as u32);
            placed.push(LayoutRawBlock {
                kind: LayoutBlockKind::Regular,
                offset: field_offset,
                size,
                alignment: align,
                field_index: Some(field_index),
                is_reference: is_ref,
            });
            content_end = content_end.max(field_end);

            // Split the consumed interval: the alignment padding before
            // the field stays free (subclasses/smaller fields may still
            // fill it — this is what reuses the klass gap), the tail
            // after the field remains free as well.
            let mut replacement = Vec::with_capacity(2);
            if field_offset > start {
                replacement.push((start, field_offset));
            }
            if field_end < end {
                replacement.push((field_end, end));
            }
            free.splice(slot..=slot, replacement);
        }

        // Instance size covers the header plus all field bytes, padded to
        // the heap word — HotSpot `FieldLayoutBuilder::epilogue`.
        let instance_size = ObjLayout::align_up(content_end, ObjLayout::HEAP_WORD_SIZE);
        let nonstatic_field_size =
            instance_size.saturating_sub(header_size) / ObjLayout::HEAP_WORD_SIZE;

        // Does the layout end with a reference?  The "last" field is the
        // occupied block with the highest offset, inherited or own.
        let mut ends_with_reference = false;
        let mut last_offset = 0usize;
        let mut consider = |block: &LayoutRawBlock| {
            if block.offset > last_offset {
                last_offset = block.offset;
                ends_with_reference = block.is_reference;
            }
        };
        if let Some(sup) = super_layout {
            for b in &sup.blocks {
                if b.occupied() {
                    consider(b);
                }
            }
        }
        for b in &placed {
            consider(b);
        }

        // Emit the full block list: header, inherited field blocks
        // (Regular -> Inherited), this class's fields, with Padding
        // blocks describing the gaps for diagnostics/oop-map consumers.
        let mut blocks: Vec<LayoutRawBlock> = Vec::with_capacity(placed.len() + 8);
        blocks.push(LayoutRawBlock {
            kind: LayoutBlockKind::Reserved,
            offset: 0,
            size: header_size,
            alignment: ObjLayout::HEAP_WORD_SIZE,
            field_index: None,
            is_reference: false,
        });
        if let Some(sup) = super_layout {
            for b in &sup.blocks {
                if b.occupied() {
                    blocks.push(LayoutRawBlock {
                        kind: LayoutBlockKind::Inherited,
                        offset: b.offset,
                        size: b.size,
                        alignment: b.alignment,
                        field_index: b.field_index,
                        is_reference: b.is_reference,
                    });
                }
            }
        }
        blocks.extend(placed);
        blocks.sort_by_key(|b| b.offset);

        // Fill gaps between occupied blocks (and the tail padding up to
        // instance_size) with Padding blocks, like HotSpot's layout dump.
        let mut result_blocks: Vec<LayoutRawBlock> = Vec::with_capacity(blocks.len() * 2);
        let mut cursor = 0usize;
        for b in blocks {
            if b.offset > cursor {
                result_blocks.push(LayoutRawBlock {
                    kind: LayoutBlockKind::Padding,
                    offset: cursor,
                    size: b.offset - cursor,
                    alignment: 1,
                    field_index: None,
                    is_reference: false,
                });
            }
            cursor = cursor.max(b.offset + b.size);
            result_blocks.push(b);
        }
        if instance_size > cursor {
            result_blocks.push(LayoutRawBlock {
                kind: LayoutBlockKind::Padding,
                offset: cursor,
                size: instance_size - cursor,
                alignment: 1,
                field_index: None,
                is_reference: false,
            });
        }

        FieldLayoutResult {
            instance_size,
            nonstatic_field_size,
            blocks: result_blocks,
            ends_with_reference,
        }
    }

    /// Computes byte offsets for static fields in class mirror/storage.
    pub fn compute_static_layout(static_base: usize, fields: &mut [FieldInfo]) -> usize {
        let mut static_indices: Vec<usize> = fields
            .iter()
            .enumerate()
            .filter(|(_, f)| f.is_static())
            .map(|(idx, _)| idx)
            .collect();

        // Sort by size descending (8, 4, 2, 1)
        static_indices.sort_by(|&a, &b| {
            let size_a = fields[a].size_in_bytes();
            let size_b = fields[b].size_in_bytes();
            size_b.cmp(&size_a).then_with(|| a.cmp(&b))
        });

        let mut current_offset = static_base;
        for field_idx in static_indices {
            let field = &mut fields[field_idx];
            let align = field.alignment();
            let size = field.size_in_bytes();

            let aligned_offset = ObjLayout::align_up(current_offset, align);
            field.set_offset(aligned_offset as u32);
            current_offset = aligned_offset + size;
        }

        ObjLayout::align_up(current_offset, ObjLayout::HEAP_WORD_SIZE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::oops::objLayout::ObjLayoutMode;

    fn field(access: u16, name: &'static [u8], desc: &'static [u8], slot: usize) -> FieldInfo<'static> {
        FieldInfo::new(access, name, desc, slot)
    }

    #[test]
    fn compressed_header_offsets_are_hotspot_compatible() {
        let layout = ObjLayout::default();
        assert_eq!(layout.mode(), ObjLayoutMode::Compressed);
        // mark word (8) + narrow klass (4) = 12 byte header; the 4 bytes
        // at offset 12 form the "klass gap" that the first suitably-sized
        // field may occupy through best-fit hole filling.
        assert_eq!(layout.header_size_in_bytes(), 12);
        assert!(layout.oop_has_klass_gap());
    }

    #[test]
    fn instance_layout_places_fields_by_size_with_klass_gap() {
        let layout = ObjLayout::default();
        let mut fields = vec![
            field(0x0001, b"i", b"I", 0),
            field(0x0001, b"l", b"J", 1),
            field(0x0001, b"r", b"Ljava/lang/Object;", 2),
        ];
        let result = FieldLayout::compute_instance_layout(&layout, 0, &mut fields);
        // HotSpot order: primitives by size desc (l then i), then oops.
        // l needs 8-byte alignment -> 16; the 4-byte hole at 12 is then
        // filled by i (klass gap reuse); r takes the tail.
        assert_eq!(fields[1].offset, 16);
        assert_eq!(fields[0].offset, 12);
        assert_eq!(fields[2].offset, 24);
        assert_eq!(result.instance_size, 32);
        assert_eq!(result.nonstatic_field_size, (32 - 12) / 8);
    }

    #[test]
    fn subclass_fills_superclass_holes() {
        let layout = ObjLayout::default();
        // Superclass { long l } leaves a 4-byte hole at offset 12.
        let mut super_fields = vec![field(0x0001, b"l", b"J", 0)];
        let super_res = FieldLayout::compute_instance_layout(&layout, 0, &mut super_fields);
        assert_eq!(super_fields[0].offset, 16);
        assert_eq!(super_res.instance_size, 24);

        // Subclass { int i }: best-fit puts i into the hole at 12 —
        // exactly what HotSpot's reconstruct_layout + fill_holes does.
        let mut sub_fields = vec![field(0x0001, b"i", b"I", 1)];
        let sub_res = FieldLayout::compute_instance_layout_inheriting(
            &layout,
            Some(&super_res),
            &mut sub_fields,
        );
        assert_eq!(sub_fields[0].offset, 12);
        // Content end stays at 24, so instance size is unchanged.
        assert_eq!(sub_res.instance_size, 24);
    }

    #[test]
    fn subclass_fields_start_after_super_instance_size() {
        let layout = ObjLayout::default();
        let mut super_fields = vec![field(0x0001, b"a", b"I", 0)];
        let super_res = FieldLayout::compute_instance_layout(&layout, 0, &mut super_fields);
        assert_eq!(super_fields[0].offset, 12);
        assert_eq!(super_res.instance_size, 16);

        let mut sub_fields = vec![field(0x0001, b"b", b"J", 1)];
        let sub_res =
            FieldLayout::compute_instance_layout(&layout, super_res.instance_size, &mut sub_fields);
        // Long field must be 8-aligned: 16 is already aligned.
        assert_eq!(sub_fields[0].offset, 16);
        assert_eq!(sub_res.instance_size, 24);
    }

    #[test]
    fn oop_fields_follow_primitives_unless_super_ends_with_oop() {
        let layout = ObjLayout::default();
        // { Object r; int i } — primitives first even though r is declared
        // first: i claims the klass gap at 12, r lands at 16.
        let mut fields = vec![
            field(0x0001, b"r", b"Ljava/lang/Object;", 0),
            field(0x0001, b"i", b"I", 1),
        ];
        let res = FieldLayout::compute_instance_layout(&layout, 0, &mut fields);
        assert_eq!(fields[1].offset, 12);
        assert_eq!(fields[0].offset, 16);
        assert!(res.ends_with_reference);

        // A subclass of a layout ending in a reference allocates its own
        // oop fields before primitives (HotSpot _super_ends_with_oop).
        let mut sub_fields = vec![
            field(0x0001, b"p", b"I", 2),
            field(0x0001, b"q", b"Ljava/lang/Object;", 3),
        ];
        let sub_res = FieldLayout::compute_instance_layout_inheriting(
            &layout,
            Some(&res),
            &mut sub_fields,
        );
        // Free tail starts at 20 (r ends at 20); q (oop, first) -> 20,
        // p (primitive) -> 24.
        assert_eq!(sub_fields[1].offset, 20);
        assert_eq!(sub_fields[0].offset, 24);
        assert_eq!(sub_res.instance_size, 32);
    }

    #[test]
    fn layout_is_deterministic() {
        let layout = ObjLayout::default();
        let run = || {
            let mut fields = vec![
                field(0x0001, b"x", b"B", 0),
                field(0x0001, b"y", b"D", 1),
                field(0x0001, b"z", b"S", 2),
                field(0x0001, b"w", b"Ljava/lang/String;", 3),
            ];
            FieldLayout::compute_instance_layout(&layout, 0, &mut fields)
        };
        let a = run();
        let b = run();
        assert_eq!(a, b);
    }
}
