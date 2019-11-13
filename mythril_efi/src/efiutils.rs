use alloc::vec::Vec;
use core::mem::MaybeUninit;
use mythril_core::allocator::FrameAllocator;
use mythril_core::error::{Error, Result};
use mythril_core::memory::{HostPhysAddr, HostPhysFrame};
use mythril_core::vm::VmServices;
use uefi::data_types::Handle;
use uefi::prelude::ResultExt;
use uefi::proto::media::file::{File, FileAttribute, FileMode, FileType};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::table::boot::{AllocateType, BootServices, MemoryType};

pub struct EfiVmServices<'a> {
    bt: &'a BootServices,
    alloc: EfiAllocator<'a>,
}

impl<'a> VmServices for EfiVmServices<'a> {
    type Allocator = EfiAllocator<'a>;
    fn allocator(&mut self) -> &mut EfiAllocator<'a> {
        &mut self.alloc
    }
    fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        read_file(self.bt, path)
    }
}

impl<'a> EfiVmServices<'a> {
    pub fn new(bt: &'a BootServices) -> Self {
        Self {
            bt: bt,
            alloc: EfiAllocator::new(bt),
        }
    }
}

pub struct EfiAllocator<'a> {
    bt: &'a BootServices,
}

impl<'a> EfiAllocator<'a> {
    pub fn new(bt: &'a BootServices) -> Self {
        EfiAllocator { bt: bt }
    }
}

impl<'a> FrameAllocator for EfiAllocator<'a> {
    fn allocate_frame(&mut self) -> Result<HostPhysFrame> {
        let ty = AllocateType::AnyPages;
        let mem_ty = MemoryType::LOADER_DATA;
        let pg = self
            .bt
            .allocate_pages(ty, mem_ty, 1)
            .log_warning()
            .map_err(|_| Error::Uefi("EfiAllocator failed to allocate frame".into()))?;

        //FIXME: For now, zero every frame we allocate
        let ptr = pg as *mut u8;
        unsafe {
            core::ptr::write_bytes(ptr, 0, 4096);
        }

        HostPhysFrame::from_start_address(HostPhysAddr::new(pg))
    }

    fn deallocate_frame(&mut self, frame: HostPhysFrame) -> Result<()> {
        self.bt
            .free_pages(frame.start_address().as_u64(), 1)
            .log_warning()
            .map_err(|_| Error::Uefi("EfiAllocator failed to deallocate frame".into()))
    }
}

//FIXME this whole function is rough
fn read_file(services: &BootServices, path: &str) -> Result<Vec<u8>> {
    let fs = uefi::table::boot::SearchType::from_proto::<SimpleFileSystem>();
    let num_handles = services
        .locate_handle(fs, None)
        .log_warning()
        .map_err(|_| Error::Uefi("Failed to get number of FS handles".into()))?;

    let mut volumes: Vec<Handle> =
        vec![unsafe { MaybeUninit::uninit().assume_init() }; num_handles];
    let _ = services
        .locate_handle(fs, Some(&mut volumes))
        .log_warning()
        .map_err(|_| Error::Uefi("Failed to read FS handles".into()))?;

    for volume in volumes.into_iter() {
        let proto = services
            .handle_protocol::<SimpleFileSystem>(volume)
            .log_warning()
            .map_err(|_| Error::Uefi("Failed to protocol for FS handle".into()))?;
        let fs = unsafe { proto.get().as_mut() }
            .ok_or(Error::NullPtr("FS Protocol ptr was NULL".into()))?;

        let mut root = fs
            .open_volume()
            .log_warning()
            .map_err(|_| Error::Uefi("Failed to open volume".into()))?;

        let handle = match root
            .open(path, FileMode::Read, FileAttribute::READ_ONLY)
            .log_warning()
        {
            Ok(f) => f,
            Err(_) => continue,
        };
        let file = handle
            .into_type()
            .log_warning()
            .map_err(|_| Error::Uefi(format!("Failed to convert file")))?;

        match file {
            FileType::Regular(mut f) => {
                info!("Reading file: {}", path);
                let mut contents = vec![];
                let mut buff = [0u8; 1024];
                while f
                    .read(&mut buff)
                    .log_warning()
                    .map_err(|_| Error::Uefi(format!("Failed to read file: {}", path)))?
                    > 0
                {
                    contents.extend_from_slice(&buff);
                }
                return Ok(contents);
            }
            _ => return Err(Error::Uefi(format!("Image file {} was a directory", path))),
        }
    }

    Err(Error::MissingFile(format!(
        "Unable to find image file {}",
        path
    )))
}