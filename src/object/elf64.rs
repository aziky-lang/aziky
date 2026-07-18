const EI_NIDENT: usize = 16;
const ELF_HEADER_SIZE: usize = 64;
const PROGRAM_HEADER_SIZE: usize = 56;

const ELF_BASE_VADDR: u64 = 0x400000;
const CODE_OFFSET: u64 = 0x1000;

#[derive(Debug, Clone, Copy)]
pub struct Elf64Header {
    pub e_ident: [u8; EI_NIDENT],
    pub e_type: u16,
    pub e_machine: u16,
    pub e_version: u32,
    pub e_entry: u64,
    pub e_phoff: u64,
    pub e_shoff: u64,
    pub e_flags: u32,
    pub e_ehsize: u16,
    pub e_phentsize: u16,
    pub e_phnum: u16,
    pub e_shentsize: u16,
    pub e_shnum: u16,
    pub e_shstrndx: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct ProgramHeader {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_paddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

impl Elf64Header {
    pub fn new(entry: u64) -> Self {
        let mut ident = [0_u8; EI_NIDENT];
        ident[0] = 0x7F;
        ident[1] = b'E';
        ident[2] = b'L';
        ident[3] = b'F';
        ident[4] = 2; // ELFCLASS64
        ident[5] = 1; // ELFDATA2LSB
        ident[6] = 1; // EV_CURRENT
        ident[7] = 0; // ELFOSABI_SYSV

        Self {
            e_ident: ident,
            e_type: 2,     // ET_EXEC
            e_machine: 62, // EM_X86_64
            e_version: 1,
            e_entry: entry,
            e_phoff: ELF_HEADER_SIZE as u64,
            e_shoff: 0,
            e_flags: 0,
            e_ehsize: ELF_HEADER_SIZE as u16,
            e_phentsize: PROGRAM_HEADER_SIZE as u16,
            e_phnum: 1,
            e_shentsize: 0,
            e_shnum: 0,
            e_shstrndx: 0,
        }
    }

    pub fn to_bytes(self) -> [u8; ELF_HEADER_SIZE] {
        let mut out = [0_u8; ELF_HEADER_SIZE];
        out[0..16].copy_from_slice(&self.e_ident);
        out[16..18].copy_from_slice(&self.e_type.to_le_bytes());
        out[18..20].copy_from_slice(&self.e_machine.to_le_bytes());
        out[20..24].copy_from_slice(&self.e_version.to_le_bytes());
        out[24..32].copy_from_slice(&self.e_entry.to_le_bytes());
        out[32..40].copy_from_slice(&self.e_phoff.to_le_bytes());
        out[40..48].copy_from_slice(&self.e_shoff.to_le_bytes());
        out[48..52].copy_from_slice(&self.e_flags.to_le_bytes());
        out[52..54].copy_from_slice(&self.e_ehsize.to_le_bytes());
        out[54..56].copy_from_slice(&self.e_phentsize.to_le_bytes());
        out[56..58].copy_from_slice(&self.e_phnum.to_le_bytes());
        out[58..60].copy_from_slice(&self.e_shentsize.to_le_bytes());
        out[60..62].copy_from_slice(&self.e_shnum.to_le_bytes());
        out[62..64].copy_from_slice(&self.e_shstrndx.to_le_bytes());
        out
    }
}

impl ProgramHeader {
    pub fn load_segment(code_size: u64) -> Self {
        let code_vaddr = ELF_BASE_VADDR + CODE_OFFSET;
        Self {
            p_type: 1,    // PT_LOAD
            p_flags: 0x5, // PF_R | PF_X
            p_offset: CODE_OFFSET,
            p_vaddr: code_vaddr,
            p_paddr: code_vaddr,
            p_filesz: code_size,
            p_memsz: code_size,
            p_align: 0x1000,
        }
    }

    pub fn to_bytes(self) -> [u8; PROGRAM_HEADER_SIZE] {
        let mut out = [0_u8; PROGRAM_HEADER_SIZE];
        out[0..4].copy_from_slice(&self.p_type.to_le_bytes());
        out[4..8].copy_from_slice(&self.p_flags.to_le_bytes());
        out[8..16].copy_from_slice(&self.p_offset.to_le_bytes());
        out[16..24].copy_from_slice(&self.p_vaddr.to_le_bytes());
        out[24..32].copy_from_slice(&self.p_paddr.to_le_bytes());
        out[32..40].copy_from_slice(&self.p_filesz.to_le_bytes());
        out[40..48].copy_from_slice(&self.p_memsz.to_le_bytes());
        out[48..56].copy_from_slice(&self.p_align.to_le_bytes());
        out
    }
}

pub fn emit_elf64_executable(code: &[u8]) -> Vec<u8> {
    let code_vaddr = ELF_BASE_VADDR + CODE_OFFSET;
    let header = Elf64Header::new(code_vaddr);
    let program_header = ProgramHeader::load_segment(code.len() as u64);

    let mut file = Vec::new();
    file.extend_from_slice(&header.to_bytes());
    file.extend_from_slice(&program_header.to_bytes());

    let desired = CODE_OFFSET as usize;
    if file.len() < desired {
        file.resize(desired, 0);
    }

    file.extend_from_slice(code);
    file
}

#[cfg(test)]
#[path = "elf64/tests.rs"]
mod tests;
