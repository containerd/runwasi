package wasmtimeoci

import (
	"fmt"
	"path/filepath"
	"strings"

	"github.com/bytecodealliance/wasmtime-go"
	"github.com/opencontainers/runtime-spec/specs-go"
)

func PrepareRootfs(spec *specs.Spec, bundle string, wasi *wasmtime.WasiConfig) (string, error) {
	rootfs := spec.Root.Path
	if rootfs == "" {
		if bundle == "" {
			return "", fmt.Errorf("no rootfs or bundle patth specified")
		}
		rootfs = filepath.Join(bundle, "rootfs")
	}

	// for _, m := range spec.Mounts {
	// 	if m.Destination == "/" {
	// 		return "", fmt.Errorf("mount destination cannot be /")
	// 	}
	// 	if err := mount.Mount(m.Source, filepath.Join(rootfs, m.Destination), m.Type, strings.Join(m.Options, ",")); err != nil {
	// 		return "", fmt.Errorf("error mounting %s: %w", m.Destination, err)
	// 	}
	// }

	if err := wasi.PreopenDir(rootfs, "/"); err != nil {
		return "", err
	}

	return rootfs, nil
}

func PrepareEnv(spec *specs.Process, wasi *wasmtime.WasiConfig) error {
	keys := make([]string, len(spec.Env))
	values := make([]string, len(spec.Env))

	for i, e := range spec.Env {
		env := strings.SplitN(e, "=", 2)
		if len(env) != 2 {
			return fmt.Errorf("invalid environment variable: %s", e)
		}

		keys[i] = env[0]
		values[i] = env[1]
	}

	wasi.SetEnv(keys, values)

	return nil
}
