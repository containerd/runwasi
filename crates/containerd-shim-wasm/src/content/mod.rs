use anyhow::{Context, Error, Result};
use serde::{Deserialize, Serialize};
use sha256::try_digest;
use std::fs::File;
use std::io::prelude::*;
use std::path::PathBuf;

#[derive(Deserialize, Serialize, PartialEq, Clone)]
pub struct Digest {
    alg: String,
    enc: String,
}

impl Digest {
    fn new(algorithm: String, encoded: String) -> Self {
        Self {
            alg: algorithm,
            enc: encoded,
        }
    }

    fn algorithm(&self) -> &str {
        &self.alg
    }

    fn encoded(&self) -> &str {
        &self.enc
    }
}

impl std::fmt::Display for Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.algorithm(), self.encoded())
    }
}

impl TryFrom<&str> for Digest {
    type Error = Error;
    fn try_from(s: &str) -> Result<Self> {
        let mut parts = s.splitn(2, ':');
        let algorithm = parts
            .next()
            .ok_or_else(|| Error::msg(format!("invalid digest format: {}", s)))?;
        let hex = parts
            .next()
            .ok_or_else(|| Error::msg(format!("invalid digest format: {}", s)))?;
        Ok(Self::new(algorithm.to_string(), hex.to_string()))
    }
}

impl TryFrom<String> for Digest {
    type Error = Error;
    fn try_from(s: String) -> Result<Self> {
        Self::try_from(s.as_str())
    }
}

impl Into<String> for Digest {
    fn into(self) -> String {
        self.algorithm().to_owned() + ":" + self.encoded()
    }
}

pub struct Store {
    dir: PathBuf,
}

#[derive(Deserialize, Serialize)]
pub struct Metadata {
    pub digest: Digest,
}

impl Store {
    pub fn new(dir: &str) -> Self {
        let p = PathBuf::from(dir);
        std::fs::create_dir_all(p.join("blobs/sha256")).unwrap();
        std::fs::create_dir_all(p.join("ingests")).unwrap();
        std::fs::create_dir_all(p.join("metadata/sha256")).unwrap();

        Self {
            dir: PathBuf::from(dir),
        }
    }

    pub fn path(&self, dgst: &Digest) -> PathBuf {
        self.dir
            .join("blobs")
            .join(dgst.algorithm())
            .join(dgst.encoded())
    }

    pub fn metadata(&self, dgst: &Digest) -> Result<Metadata> {
        let path = self
            .dir
            .join("metadata")
            .join(dgst.algorithm())
            .join(dgst.encoded());

        let mut file = File::open(path).context("could not open metadata file")?;
        let mut buf = String::new();
        file.read_to_string(&mut buf)
            .context("could not read metadata file")?;

        Ok(serde_json::from_str(&buf).context("could not parse metadata file")?)
    }

    pub fn write_metadata(&self, dgst: &Digest, data: &Metadata) -> Result<()> {
        let path = self
            .dir
            .join("metadata")
            .join(dgst.algorithm())
            .join(dgst.encoded());

        let f = File::create(path)?;
        serde_json::to_writer(f, data)?;
        Ok(())
    }

    pub fn writer(&mut self, id: String) -> Result<ContentWriter> {
        let ingest_path = self.dir.join("ingests").join(id);
        let target = self.dir.join("blobs");
        let f = File::create(&ingest_path)?;
        let cw = ContentWriter {
            f,
            ingest_path,
            target,
        };
        Ok(cw)
    }
}

pub struct ContentWriter {
    f: File,
    ingest_path: PathBuf,
    target: PathBuf,
}

impl ContentWriter {
    pub fn write(&mut self, data: &[u8]) -> Result<usize> {
        self.f.try_clone()?.write(data).context("write")
    }

    pub fn commit(&mut self, expected: Option<Digest>) -> Result<Digest> {
        self.f.try_clone()?.flush().context("flush")?;

        let dgst = try_digest(self.ingest_path.as_path()).context("digest")?;
        let d: Digest = ("sha256:".to_owned() + &dgst).try_into()?;

        if let Some(ex) = expected {
            if d != ex {
                return Err(Error::msg(format!("digest mismatch: {} != {}", d, ex)));
            }
        }

        std::fs::rename(
            &self.ingest_path,
            &self.target.join(d.algorithm()).join(d.encoded()),
        )?;
        Ok(d)
    }
}
