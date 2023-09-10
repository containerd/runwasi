(module
    ;; Import the required proc_exit WASI function which terminates the program with an exit code.
    ;; The function signature for proc_exit is:
    ;; (exit_code: i32) -> !
    (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))
    (memory 1)
    (export "memory" (memory 0))
    (func $main (export "_start")
        (call $proc_exit (i32.const 42))
        unreachable
    )
)
