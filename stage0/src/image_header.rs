// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use abi::ImageHeader;

extern "C" {
    static IMAGEA: abi::ImageHeader;
}

pub struct Image(&'static ImageHeader);

pub fn get_image_a() -> Option<Image> {
    // Taking the reference to our supposed imagea
    let imagea = unsafe { &IMAGEA };

    let img = Image(imagea);

    if !img.validate() {
        return None;
    }

    Some(img)
}

impl Image {
    fn get_img_start(&self) -> u32 {
        self.0 as *const ImageHeader as u32
    }

    /// Make sure all of the image flash is programmed
    pub fn validate(&self) -> bool {
        let img_start = self.get_img_start();

        // Start by making sure the region is actually programmed
        let valid = lpc55_romapi::validate_programmed(img_start, 0x200);

        if !valid {
            return false;
        }

        // Next make sure the marked image length is programmed
        let valid = lpc55_romapi::validate_programmed(
            img_start,
            (self.0.total_image_len + 0x1ff) & !(0x1ff),
        );

        if !valid {
            return false;
        }

        if self.0.magic != abi::HEADER_MAGIC {
            return false;
        }

        return true;
    }

    pub fn get_vectors(&self) -> u32 {
        self.0.vector
    }

    pub fn get_pc(&self) -> u32 {
        let vec = self.get_vectors();

        unsafe { core::ptr::read_volatile((vec + 0x4) as *const u32) }
    }

    pub fn get_sp(&self) -> u32 {
        let vec = self.get_vectors();

        unsafe { core::ptr::read_volatile(vec as *const u32) }
    }

    pub fn get_sau_entry<'a>(&self, i: usize) -> Option<&'a abi::SAUEntry> {
        if i > 8 {
            return None;
        }

        Some(&self.0.sau_entries[i])
    }
}
