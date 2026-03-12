use crate::lx::LxFile;

pub struct Loader {
    // Memory mapping info will go here
}

impl Loader {
    pub fn new() -> Self {
        Loader {}
    }

    pub fn load(&mut self, _lx_file: &LxFile) -> Result<(), &'static str> {
        // Stub for mapping LX file into memory
        Ok(())
    }
}
