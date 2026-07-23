use std::{
    env, fs,
    io::{self, Write},
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const CHECK_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Generation {
    device: u64,
    inode: u64,
}

impl Generation {
    fn read(path: &Path) -> io::Result<Self> {
        let metadata = fs::metadata(path)?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    fn parse(contents: &str) -> io::Result<Self> {
        let mut fields = contents.split_whitespace();
        let device = fields
            .next()
            .and_then(|field| field.parse().ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing device"))?;
        let inode = fields
            .next()
            .and_then(|field| field.parse().ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing inode"))?;
        Ok(Self { device, inode })
    }
}

pub(crate) struct RestartCoordinator {
    executable: PathBuf,
    generation: Generation,
    marker: PathBuf,
    next_check: Instant,
    check_interval: Duration,
}

impl RestartCoordinator {
    pub(crate) fn start() -> io::Result<Self> {
        Self::at(
            env::current_exe()?,
            coordination_directory()?,
            CHECK_INTERVAL,
        )
    }

    fn at(
        executable: PathBuf,
        coordination_directory: PathBuf,
        check_interval: Duration,
    ) -> io::Result<Self> {
        let generation = Generation::read(&executable)?;
        fs::create_dir_all(&coordination_directory)?;
        let marker = coordination_directory.join(format!(
            "generation-{:016x}",
            executable_path_hash(&executable)
        ));
        let coordinator = Self {
            executable,
            generation,
            marker,
            next_check: Instant::now() + check_interval,
            check_interval,
        };
        coordinator.publish()?;
        Ok(coordinator)
    }

    pub(crate) fn poll(&mut self) -> io::Result<Option<PathBuf>> {
        if Instant::now() < self.next_check {
            return Ok(None);
        }
        self.next_check = Instant::now() + self.check_interval;
        let generation = match fs::read_to_string(&self.marker) {
            Ok(contents) => Generation::parse(&contents)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.publish()?;
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        Ok((generation != self.generation).then(|| self.executable.clone()))
    }

    fn publish(&self) -> io::Result<()> {
        let mut file = atomic_write_file::AtomicWriteFile::open(&self.marker)?;
        writeln!(file, "{} {}", self.generation.device, self.generation.inode)?;
        file.commit()
    }
}

fn coordination_directory() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("XDG_RUNTIME_DIR") {
        return Ok(PathBuf::from(path).join("hunkle"));
    }
    if let Some(path) = env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(path).join("hunkle").join("runtime"));
    }
    let home = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "home directory is unavailable"))?;
    Ok(PathBuf::from(home)
        .join(".local")
        .join("state")
        .join("hunkle")
        .join("runtime"))
}

fn executable_path_hash(path: &Path) -> u64 {
    path.as_os_str()
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_a_replaced_executable_without_restarting_its_new_generation() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("hunkle");
        let markers = directory.path().join("markers");
        fs::write(&executable, "old").unwrap();
        let mut old =
            RestartCoordinator::at(executable.clone(), markers.clone(), Duration::ZERO).unwrap();

        let replacement = directory.path().join("replacement");
        fs::write(&replacement, "new").unwrap();
        fs::rename(replacement, &executable).unwrap();
        let mut new = RestartCoordinator::at(executable.clone(), markers, Duration::ZERO).unwrap();

        assert_eq!(old.poll().unwrap(), Some(executable));
        assert_eq!(new.poll().unwrap(), None);
    }

    #[test]
    fn keeps_separate_executable_paths_on_separate_channels() {
        let directory = tempfile::tempdir().unwrap();
        let markers = directory.path().join("markers");
        let installed = directory.path().join("installed-hunkle");
        let debug = directory.path().join("debug-hunkle");
        fs::write(&installed, "installed").unwrap();
        fs::write(&debug, "debug").unwrap();
        let mut installed =
            RestartCoordinator::at(installed, markers.clone(), Duration::ZERO).unwrap();

        RestartCoordinator::at(debug, markers, Duration::ZERO).unwrap();

        assert_eq!(installed.poll().unwrap(), None);
    }
}
