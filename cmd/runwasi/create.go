package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"syscall"

	"github.com/bytecodealliance/wasmtime-go"
	"github.com/cpuguy83/runwasi/wasmtimeoci"
	"github.com/moby/sys/mount"
	"github.com/opencontainers/runtime-spec/specs-go"
	cli "github.com/urfave/cli/v2"
	"golang.org/x/sys/unix"
)

const modulesDir = "wasi.modules.d"

func addCreateCmd(app *cli.App) {
	app.Commands = append(app.Commands, &cli.Command{
		Name:  "create",
		Usage: "create a new container",
		Flags: []cli.Flag{
			&cli.StringFlag{Name: "bundle", Usage: "path to the root of the bundle directory"},
			&cli.StringFlag{Name: "pid-file", Usage: "path to write the process id to"},
		},
		Action: runCreateCmd,
		Subcommands: []*cli.Command{
			{
				Name:   "init",
				Hidden: true,
				Action: runCreateInitCmd,
			},
		},
	})

}

func runCreateCmd(cmd *cli.Context) error {
	exe, err := os.Executable()
	if err != nil {
		return err
	}

	createInit := exec.Command(exe, "create", "--bundle="+cmd.String("bundle"), "--pid-file="+cmd.String("pid-file"), "init")
	createInit.SysProcAttr = &syscall.SysProcAttr{
		Cloneflags: syscall.CLONE_NEWNS,
	}
	createInit.Env = append(os.Environ(), "_RUNWASI_PHASE=1")

	if err := unix.Mkfifo(filepath.Join(cmd.String("bundle"), "__init_pipe"), 0600); err != nil {
		return err
	}

	pipe, err := os.OpenFile(filepath.Join(cmd.String("bundle"), "__init_pipe"), os.O_RDONLY, 0)
	if err != nil {
		return err
	}
	defer pipe.Close()

	createInit.ExtraFiles = append(createInit.ExtraFiles, pipe)

	bundle := cmd.String("bundle")

	specData, err := os.ReadFile(filepath.Join(bundle, "config.json"))
	if err != nil {
		return fmt.Errorf("error reading container spec: %w", err)
	}

	var spec specs.Spec
	if err := json.Unmarshal(specData, &spec); err != nil {
		return fmt.Errorf("error parsing container spec: %w", err)
	}

	for _, ns := range spec.Linux.Namespaces {
		if ns.Type == specs.NetworkNamespace && ns.Path != "" {
			f, err := os.OpenFile(ns.Path, os.O_RDONLY, 0)
			if err != nil {
				return fmt.Errorf("error opening network namespace: %w", err)
			}
			defer f.Close()
			createInit.ExtraFiles = append(createInit.ExtraFiles, f)
			break
		}
	}

	createInit.Stdin = os.Stdin
	createInit.Stdout = os.Stdout
	createInit.Stderr = os.Stderr

	// TODO: Set other namespaces? For the most part I think this would mostly be extra safeguards more than something we actually need.

	if err := createInit.Start(); err != nil {
		return fmt.Errorf("error starting init: %w", err)
	}

	return nil
}

func runCreateInitCmd(cmd *cli.Context) error {
	// We should always be in a new mount namespace here, setup by the parent process
	// We use `rslave` here because we want mounts from the host to propagate in, but not the otherway around.
	if err := mount.MakeRSlave("/"); err != nil {
		return fmt.Errorf("error making private mount namespace: %w", err)
	}

	bundle := cmd.String("bundle")

	specData, err := os.ReadFile(filepath.Join(bundle, "config.json"))
	if err != nil {
		return fmt.Errorf("error reading container spec: %w", err)
	}

	var spec specs.Spec
	if err := json.Unmarshal(specData, &spec); err != nil {
		return fmt.Errorf("error parsing container spec: %w", err)
	}

	if spec.Process.Terminal {
		return fmt.Errorf("tty not supported")
	}

	engine := wasmtime.NewEngine()

	linker := wasmtime.NewLinker(engine)
	store := wasmtime.NewStore(engine)

	if err := linker.DefineWasi(); err != nil {
		panic(err)
	}

	modsLs, err := os.ReadDir(modulesDir)
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return err
	}

	for _, mod := range modsLs {
		if mod.IsDir() {
			continue
		}

		m, err := wasmtime.NewModuleFromFile(engine, filepath.Join(modulesDir, mod.Name()))
		if err != nil {
			return err
		}
		if err := linker.DefineModule(store, mod.Name(), m); err != nil {
			return err
		}
	}

	mod, err := wasmtime.NewModuleFromFile(engine, filepath.Join(bundle, spec.Process.Args[0]))
	if err != nil {
		return err
	}

	wasi := wasmtime.NewWasiConfig()

	if err := wasmtimeoci.PrepareEnv(spec.Process, wasi); err != nil {
		return err
	}
	if _, err := wasmtimeoci.PrepareRootfs(&spec, bundle, wasi); err != nil {
		return err
	}

	wasi.SetArgv(spec.Process.Args)

	wasi.InheritStdin()
	wasi.InheritStdout()
	wasi.InheritStderr()

	store.SetWasi(wasi)

	instance, err := linker.Instantiate(store, mod)
	if err != nil {
		return err
	}

	if err := os.WriteFile(cmd.String("pid-file"), []byte(fmt.Sprintf("%d", os.Getpid())), 0600); err != nil {
		return err
	}

	fn := instance.GetExport(store, "_start").Func()
	if fn == nil {
		return fmt.Errorf("%w: module start function not found", os.ErrNotExist)
	}
	_, err = fn.Call(store)

	return err
}
