#[macro_export]
macro_rules! cfg_windows {
    ($($item:item)*) => {
        $(
            #[cfg(windows)]
            $item
        )*
    }
}

#[macro_export]
macro_rules! cfg_unix {
    ($($item:item)*) => {
        $(
            #[cfg(unix)]
            $item
        )*
    }
}
