package main

import (
	"fmt"
	"os"
	"strconv"
	"strings"
	"time"
)

func main() {
	defer fmt.Fprintln(os.Stderr, "exiting")
	if len(os.Args) == 1 {
		fmt.Println("Hello fromw wasm!")
		return
	}

	switch os.Args[1] {
	case "sleep":
		dur, err := time.ParseDuration(os.Args[2])
		if err != nil {
			seconds, err := strconv.Atoi(os.Args[2])
			if err != nil {
				panic(err)
			}
			dur = time.Duration(seconds) * time.Second
		}
		time.Sleep(dur)
	case "echo":
		fmt.Println(strings.Join(os.Args[2:], " "))
	case "exit":
		code, err := strconv.Atoi(os.Args[2])
		if err != nil {
			panic(err)
		}
		os.Exit(code)
	case "daemon":
		for {
			fmt.Println("Hello from wasm!")
			time.Sleep(time.Second)
		}
	default:
		fmt.Fprintln(os.Stderr, "unknown command", os.Args[1])
		os.Exit(1)
	}

}
