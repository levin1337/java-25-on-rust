/*
 * Port of OpenJDK 25 HotSpot FieldLayoutBuilder (share/classfile/fieldLayoutBuilder.hpp, fieldLayoutBuilder.cpp)
 *
 * Computes memory layouts and byte offsets for non-static and static fields:
 * - Proper header alignment based on ObjLayout (compact, compressed, or uncompressed)
 * - Klass gap filling for 4-byte fields when compressed class pointers are enabled
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldLayoutResult {
    pub instance_size: usize,
    pub nonstatic_field_size: usize,
    pub blocks: Vec<LayoutRawBlock>,
}

pub struct FieldLayout;

impl FieldLayout {
    /// Computes byte offsets for instance (non-static) fields.
    ///
    /// Respects superclass instance layout, object header size, klass gap (if present),
    /// and field alignment constraints.
    pub fn compute_instance_layout(
        layout: &ObjLayout,
        super_instance_size: usize,
        fields: &mut [FieldInfo],
    ) -> FieldLayoutResult {
        let header_size = layout.header_size_in_bytes();
        let mut start_offset = if super_instance_size > 0 {
            super_instance_size
        } else {
            header_size
        };

        let mut blocks = Vec::new();
        if super_instance_size > 0 {
            blocks.push(LayoutRawBlock {
                kind: LayoutBlockKind::Inherited,
                offset: 0,
                size: super_instance_size,
                alignment: ObjLayout::HEAP_WORD_SIZE,
                field_index: None,
            });
        } else {
            blocks.push(LayoutRawBlock {
                kind: LayoutBlockKind::Reserved,
                offset: 0,
                size: header_size,
                alignment: ObjLayout::HEAP_WORD_SIZE,
                field_index: None,
            });
        }

        // Check if there is a 4-byte klass gap to allocate into (only for base class extending Object)
        let klass_gap_available = layout.oop_has_klass_gap() && super_instance_size == 0;
        let klass_gap_offset = header_size;
        let klass_gap_size = 4;

        if klass_gap_available {
            start_offset = header_size + klass_gap_size;
        }

        // Collect non-static fields
        let mut nonstatic_indices: Vec<usize> = fields
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.is_static())
            .map(|(idx, _)| idx)
            .collect();

        // If klass gap is available, try to place one 4-byte field in it
        if klass_gap_available {
            if let Some(pos) = nonstatic_indices
                .iter()
                .position(|&idx| fields[idx].size_in_bytes() == 4)
            {
                let field_idx = nonstatic_indices.remove(pos);
                fields[field_idx].set_offset(klass_gap_offset as u32);
                blocks.push(LayoutRawBlock {
                    kind: LayoutBlockKind::Regular,
                    offset: klass_gap_offset,
                    size: 4,
                    alignment: 4,
                    field_index: Some(field_idx),
                });
            }
        }

        // Sort remaining fields by size descending (8, 4, 2, 1) to minimize alignment holes
        nonstatic_indices.sort_by(|&a, &b| {
            let size_a = fields[a].size_in_bytes();
            let size_b = fields[b].size_in_bytes();
            size_b.cmp(&size_a).then_with(|| a.cmp(&b))
        });

        let mut current_offset = start_offset;
        for field_idx in nonstatic_indices {
            let field = &mut fields[field_idx];
            let align = field.alignment();
            let size = field.size_in_bytes();

            let aligned_offset = ObjLayout::align_up(current_offset, align);
            if aligned_offset > current_offset {
                blocks.push(LayoutRawBlock {
                    kind: LayoutBlockKind::Padding,
                    offset: current_offset,
                    size: aligned_offset - current_offset,
                    alignment: 1,
                    field_index: None,
                });
            }

            field.set_offset(aligned_offset as u32);
            blocks.push(LayoutRawBlock {
                kind: LayoutBlockKind::Regular,
                offset: aligned_offset,
                size,
                alignment: align,
                field_index: Some(field_idx),
            });

            current_offset = aligned_offset + size;
        }

        // Align total instance size to heap word boundary (8 bytes)
        let instance_size = ObjLayout::align_up(current_offset, ObjLayout::HEAP_WORD_SIZE);
        if instance_size > current_offset {
            blocks.push(LayoutRawBlock {
                kind: LayoutBlockKind::Padding,
                offset: current_offset,
                size: instance_size - current_offset,
                alignment: 1,
                field_index: None,
            });
        }

        let nonstatic_field_size =
            (instance_size.saturating_sub(header_size)) / ObjLayout::HEAP_WORD_SIZE;

        FieldLayoutResult {
            instance_size,
            nonstatic_field_size,
            blocks,
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
