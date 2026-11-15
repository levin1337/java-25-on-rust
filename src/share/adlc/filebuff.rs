/*
 * OpenJDK 25 HotSpot adlc/ port
 *
 * FileBuff: whole-file buffer with line tracking, mirroring filebuff.cpp.
 * The buffer layout replicates the C++ sentinel scheme:
 *   buf[0] = '\n' (sentinel), then file bytes, then '\n', '\0'.
 * Lines returned by get_line() include their trailing '\n'.
 */

pub struct FileBuff {
    pub buf: Vec<u8>,
    pub bufeol: usize,   // index of '\n' ending the last returned line
    pub bufmax: usize,   // index of the sentinel '\n' appended after content
    pub linenum: usize,
    pub name: String,
}

impl FileBuff {
    pub fn new(content: &[u8], name: &str) -> FileBuff {
        let mut buf = Vec::with_capacity(content.len() + 4);
        buf.push(b'\n');
        buf.extend_from_slice(content);
        let bufmax = buf.len();
        buf.push(b'\n');
        buf.push(0);
        FileBuff {
            buf,
            bufeol: 0,
            bufmax,
            linenum: 0,
            name: name.to_string(),
        }
    }

    pub fn from_file(path: &str) -> Result<FileBuff, String> {
        let content = std::fs::read(path).map_err(|e| format!("{}: {}", path, e))?;
        Ok(FileBuff::new(&content, path))
    }

    // Returns start index of next line, or None at EOF.
    // After the call, the line spans [retval, bufeol] with buf[bufeol]=='\n'.
    pub fn get_line(&mut self) -> Option<usize> {
        if self.bufeol >= self.bufmax {
            return None;
        }
        self.linenum += 1;
        self.bufeol += 1;
        let retval = self.bufeol;
        if self.buf[retval] == 0 {
            return None;
        }
        while self.buf[self.bufeol] != b'\n' {
            self.bufeol += 1;
        }
        Some(retval)
    }
}
