use std::iter::{FilterMap, Map};
use std::path::{Path, PathBuf};
use std::slice::Iter;
use std::str::Split;

use oci_spec::runtime::Spec;

// Ideally this would be `impl Iterator<Item = (&str, &str)>`
// but we can't return `impl ...` in traits.
pub type EnvIterator<'a> = Map<Iter<'a, String>, fn(&String) -> (&str, &str)>;

// Ideally this would be `impl Iterator<Item = PathBuf>`
// but we can't return `impl ...` in traits.
pub type FindInPathIterator<'a> =
    FilterMap<Split<'a, char>, Box<dyn FnMut(&str) -> Option<PathBuf>>>;

pub trait RuntimeContext {
    // ctx.args() returns arguments from the runtime spec process field, including the
    // path to the entrypoint executable.
    fn args(&self) -> &[String];

    // ctx.entrypoint() returns the entrypoint path from arguments on the runtime
    // spec process field.
    fn entrypoint(&self) -> Option<&Path>;

    // ctx.module() returns the module path and exported function name to be called
    // as a (&Path, &str) tuple, obtained from the arguments on the runtime spec process
    // field. The first argument will be the module name and the default function name
    // is "_start".
    //
    // If there is a '#' in the argument it will split the string returning the first part
    // as the module name and the second part as the function name.
    //
    // example: "/app/module.wasm#function" will return the tuple
    // (Path::new("/app/module.wasm"), "function")
    //
    // If there are no arguments then it will return (Path::new(""), "_start")
    fn module(&self) -> (&Path, &str);

    // ctx.envs() returns the environment variables from the runtime spec process field
    // as an `impl Iterator<Item = (&str, &str)>` representing the variable name and value
    // respectively.
    fn envs(&self) -> EnvIterator;

    // ctx.find_in_path("file.wasm") will try to find "file.wasm" using the process PATH
    // environment variable for its resolution. It returns an `impl Iterator<Item = PathBuf>`
    // with the canonical path of all found files. This function does not impose any
    // requirement other than the file existing. Extra requirements, like executable mode,
    // can be added by filtering the iterator.
    fn find_in_path(&self, file: impl AsRef<Path>) -> FindInPathIterator;
}

impl RuntimeContext for &Spec {
    fn args(&self) -> &[String] {
        self.process()
            .as_ref()
            .and_then(|p| p.args().as_ref())
            .map(|a| a.as_slice())
            .unwrap_or_default()
    }

    fn entrypoint(&self) -> Option<&Path> {
        self.args().first().map(Path::new)
    }

    fn module(&self) -> (&Path, &str) {
        let arg0 = self.args().first().map(String::as_str).unwrap_or("");
        let (module, method) = arg0.split_once('#').unwrap_or((arg0, "_start"));
        (Path::new(module), method)
    }

    fn envs(&self) -> EnvIterator {
        fn split_once(s: &String) -> (&str, &str) {
            s.split_once('=').unwrap_or((s, ""))
        }

        self.process()
            .as_ref()
            .and_then(|p| p.env().as_ref())
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(split_once)
    }

    fn find_in_path(&self, file: impl AsRef<Path>) -> FindInPathIterator {
        let executable = file.as_ref().to_owned();
        let filter = Box::new(move |p: &str| -> Option<PathBuf> {
            let path = Path::new(p).canonicalize().ok()?;
            let path = path.is_dir().then_some(path)?.join(&executable);
            let path = path.canonicalize().ok()?;
            path.is_file().then_some(path)
        });

        let path_iter = if file.as_ref().components().count() > 1 {
            "."
        } else {
            self.envs()
                .find(|(key, _)| *key == "PATH")
                .unwrap_or(("", "."))
                .1
        };

        path_iter.split(':').filter_map(filter)
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use oci_spec::runtime::{ProcessBuilder, RootBuilder, SpecBuilder};

    use super::*;

    #[test]
    fn test_get_args() -> Result<()> {
        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(
                ProcessBuilder::default()
                    .cwd("/")
                    .args(vec!["hello.wat".to_string()])
                    .build()?,
            )
            .build()?;
        let spec = &spec;

        let args = spec.args();
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], "hello.wat");

        Ok(())
    }

    #[test]
    fn test_get_args_return_empty() -> Result<()> {
        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(ProcessBuilder::default().cwd("/").args(vec![]).build()?)
            .build()?;
        let spec = &spec;

        let args = spec.args();
        assert_eq!(args.len(), 0);

        Ok(())
    }

    #[test]
    fn test_get_args_returns_all() -> Result<()> {
        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(
                ProcessBuilder::default()
                    .cwd("/")
                    .args(vec![
                        "hello.wat".to_string(),
                        "echo".to_string(),
                        "hello".to_string(),
                    ])
                    .build()?,
            )
            .build()?;
        let spec = &spec;

        let args = spec.args();
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], "hello.wat");
        assert_eq!(args[1], "echo");
        assert_eq!(args[2], "hello");

        Ok(())
    }

    #[test]
    fn test_get_module_returns_none_when_not_present() -> Result<()> {
        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(ProcessBuilder::default().cwd("/").args(vec![]).build()?)
            .build()?;
        let spec = &spec;

        let (module, _) = spec.module();
        assert!(module.as_os_str().is_empty());

        Ok(())
    }

    #[test]
    fn test_get_module_returns_function() -> Result<()> {
        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(
                ProcessBuilder::default()
                    .cwd("/")
                    .args(vec![
                        "hello.wat#foo".to_string(),
                        "echo".to_string(),
                        "hello".to_string(),
                    ])
                    .build()?,
            )
            .build()?;
        let spec = &spec;

        let (module, function) = spec.module();
        assert_eq!(module, Path::new("hello.wat"));
        assert_eq!(function, "foo");

        Ok(())
    }

    #[test]
    fn test_get_module_returns_start() -> Result<()> {
        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(
                ProcessBuilder::default()
                    .cwd("/")
                    .args(vec![
                        "/root/hello.wat".to_string(),
                        "echo".to_string(),
                        "hello".to_string(),
                    ])
                    .build()?,
            )
            .build()?;
        let spec = &spec;

        let (module, function) = spec.module();
        assert_eq!(module, Path::new("/root/hello.wat"));
        assert_eq!(function, "_start");

        Ok(())
    }
}
