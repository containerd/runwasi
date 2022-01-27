package main

/*
#cgo CFLAGS: -Wall
extern void hook();
void __attribute__((constructor)) init(void) {
	hook();
}
*/
import "C"

import (
	"fmt"
	"os"

	cli "github.com/urfave/cli/v2"
)

func main() {
	app := &cli.App{
		Name:        "runwasi",
		Description: "runwasi is a mostly runc compatible-ish tool to run wasi applications for use with containerd",
		Action: func(c *cli.Context) error {
			return cli.ShowAppHelp(c)
		},
	}

	app.Flags = []cli.Flag{
		&cli.BoolFlag{Name: "debug", Usage: "enable debug mode"},
		&cli.StringFlag{Name: "root", Usage: "root directory for storing container state"},
		&cli.StringFlag{Name: "version", Aliases: []string{"v"}, Usage: "print version information"},
	}

	addCreateCmd(app)

	if err := app.Run(os.Args); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
