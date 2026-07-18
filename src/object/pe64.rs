use crate::target::{WINDOWS_IAT_RVA, WINDOWS_IMAGE_BASE, WindowsImport};

const FILE_ALIGNMENT: u32 = 0x200;
const SECTION_ALIGNMENT: u32 = 0x1000;
const HEADERS_SIZE: u32 = 0x400;
const TEXT_RVA: u32 = 0x1000;

fn align_up(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn build_import_section() -> (Vec<u8>, u32, u32) {
    let names = WindowsImport::NAMES;
    let thunk_bytes = (names.len() + 1) * 8;
    let iat_offset = 0usize;
    let ilt_offset = thunk_bytes;
    let descriptor_offset = (ilt_offset + thunk_bytes + 7) & !7;
    let mut section = vec![0; descriptor_offset + 40];
    let dll_offset = section.len();
    section.extend_from_slice(b"KERNEL32.dll\0");

    let mut name_offsets = Vec::new();
    for name in names {
        if section.len() % 2 != 0 {
            section.push(0);
        }
        name_offsets.push(section.len());
        section.extend_from_slice(&0_u16.to_le_bytes());
        section.extend_from_slice(name.as_bytes());
        section.push(0);
    }

    let idata_rva = WINDOWS_IAT_RVA as u32;
    for (index, name_offset) in name_offsets.into_iter().enumerate() {
        let name_rva = u64::from(idata_rva) + name_offset as u64;
        put_u64(&mut section, iat_offset + index * 8, name_rva);
        put_u64(&mut section, ilt_offset + index * 8, name_rva);
    }
    put_u32(
        &mut section,
        descriptor_offset,
        idata_rva + ilt_offset as u32,
    );
    put_u32(
        &mut section,
        descriptor_offset + 12,
        idata_rva + dll_offset as u32,
    );
    put_u32(
        &mut section,
        descriptor_offset + 16,
        idata_rva + iat_offset as u32,
    );
    (section, idata_rva + descriptor_offset as u32, 40)
}

pub fn emit_pe64_executable(code: &[u8]) -> Vec<u8> {
    let (imports, import_directory_rva, import_directory_size) = build_import_section();
    let text_raw_size = align_up(code.len() as u32, FILE_ALIGNMENT);
    let idata_raw_size = align_up(imports.len() as u32, FILE_ALIGNMENT);
    let text_raw = HEADERS_SIZE;
    let idata_raw = text_raw + text_raw_size;
    let size_of_image = align_up(
        WINDOWS_IAT_RVA as u32 + imports.len() as u32,
        SECTION_ALIGNMENT,
    );

    let mut out = vec![0; (idata_raw + idata_raw_size) as usize];
    put_u16(&mut out, 0, 0x5a4d); // MZ
    put_u32(&mut out, 0x3c, 0x80);
    let pe = 0x80usize;
    out[pe..pe + 4].copy_from_slice(b"PE\0\0");
    let coff = pe + 4;
    put_u16(&mut out, coff, 0x8664);
    put_u16(&mut out, coff + 2, 2);
    put_u16(&mut out, coff + 16, 240);
    put_u16(&mut out, coff + 18, 0x0023);

    let optional = coff + 20;
    put_u16(&mut out, optional, 0x020b);
    out[optional + 2] = 1;
    put_u32(&mut out, optional + 4, text_raw_size);
    put_u32(&mut out, optional + 8, idata_raw_size);
    put_u32(&mut out, optional + 16, TEXT_RVA);
    put_u32(&mut out, optional + 20, TEXT_RVA);
    put_u64(&mut out, optional + 24, WINDOWS_IMAGE_BASE);
    put_u32(&mut out, optional + 32, SECTION_ALIGNMENT);
    put_u32(&mut out, optional + 36, FILE_ALIGNMENT);
    put_u16(&mut out, optional + 40, 6);
    put_u16(&mut out, optional + 48, 6);
    put_u32(&mut out, optional + 56, size_of_image);
    put_u32(&mut out, optional + 60, HEADERS_SIZE);
    put_u16(&mut out, optional + 68, 3); // console subsystem
    put_u16(&mut out, optional + 70, 0x0100); // NX compatible; fixed image
    put_u64(&mut out, optional + 72, 1 << 20);
    put_u64(&mut out, optional + 80, 0x1000);
    put_u64(&mut out, optional + 88, 1 << 20);
    put_u64(&mut out, optional + 96, 0x1000);
    put_u32(&mut out, optional + 108, 16);
    put_u32(&mut out, optional + 120, import_directory_rva);
    put_u32(&mut out, optional + 124, import_directory_size);
    put_u32(&mut out, optional + 208, WINDOWS_IAT_RVA as u32);
    put_u32(
        &mut out,
        optional + 212,
        ((WindowsImport::NAMES.len() + 1) * 8) as u32,
    );

    let sections = optional + 240;
    out[sections..sections + 5].copy_from_slice(b".text");
    put_u32(&mut out, sections + 8, code.len() as u32);
    put_u32(&mut out, sections + 12, TEXT_RVA);
    put_u32(&mut out, sections + 16, text_raw_size);
    put_u32(&mut out, sections + 20, text_raw);
    put_u32(&mut out, sections + 36, 0x6000_0020);

    let idata = sections + 40;
    out[idata..idata + 6].copy_from_slice(b".idata");
    put_u32(&mut out, idata + 8, imports.len() as u32);
    put_u32(&mut out, idata + 12, WINDOWS_IAT_RVA as u32);
    put_u32(&mut out, idata + 16, idata_raw_size);
    put_u32(&mut out, idata + 20, idata_raw);
    put_u32(&mut out, idata + 36, 0xc000_0040);

    out[text_raw as usize..text_raw as usize + code.len()].copy_from_slice(code);
    out[idata_raw as usize..idata_raw as usize + imports.len()].copy_from_slice(&imports);
    out
}

pub fn emit_coff_relocatable(code: &[u8]) -> Vec<u8> {
    const HEADER_SIZE: usize = 20 + 40;
    let symbol_table = HEADER_SIZE + code.len();
    let long_name = b"aziky_program_entry\0";
    let mut out = vec![0; HEADER_SIZE];
    put_u16(&mut out, 0, 0x8664);
    put_u16(&mut out, 2, 1);
    put_u32(&mut out, 8, symbol_table as u32);
    put_u32(&mut out, 12, 2);

    out[20..25].copy_from_slice(b".text");
    put_u32(&mut out, 20 + 16, code.len() as u32);
    put_u32(&mut out, 20 + 20, HEADER_SIZE as u32);
    put_u32(&mut out, 20 + 36, 0x6000_0020);
    out.extend_from_slice(code);

    let mut start = [0_u8; 18];
    start[..6].copy_from_slice(b"_start");
    start[12..14].copy_from_slice(&1_i16.to_le_bytes());
    start[14..16].copy_from_slice(&0x20_u16.to_le_bytes());
    start[16] = 2;
    out.extend_from_slice(&start);

    let mut entry = [0_u8; 18];
    entry[4..8].copy_from_slice(&4_u32.to_le_bytes());
    entry[12..14].copy_from_slice(&1_i16.to_le_bytes());
    entry[14..16].copy_from_slice(&0x20_u16.to_le_bytes());
    entry[16] = 2;
    out.extend_from_slice(&entry);
    out.extend_from_slice(&(4_u32 + long_name.len() as u32).to_le_bytes());
    out.extend_from_slice(long_name);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pe_has_deterministic_headers_and_imports() {
        let first = emit_pe64_executable(&[0xc3]);
        let second = emit_pe64_executable(&[0xc3]);
        assert_eq!(first, second);
        assert_eq!(&first[..2], b"MZ");
        assert!(first.windows(12).any(|bytes| bytes == b"KERNEL32.dll"));
        assert!(first.windows(11).any(|bytes| bytes == b"ExitProcess"));
    }

    #[test]
    fn coff_object_has_text_and_external_symbols() {
        let object = emit_coff_relocatable(&[0xc3]);
        assert_eq!(u16::from_le_bytes([object[0], object[1]]), 0x8664);
        assert!(object.windows(6).any(|bytes| bytes == b"_start"));
        assert!(
            object
                .windows(19)
                .any(|bytes| bytes == b"aziky_program_entry")
        );
    }
}
