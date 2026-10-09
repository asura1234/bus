pub(crate) mod actor;
#[cfg(unix)]
pub(crate) mod fd;

#[cfg(test)]
mod spawn {
    #[cfg(unix)]
    mod unix {
        #[cfg(all(test, target_os = "linux"))]
        mod tests {
            include!("../runtime/tests/pty_spawn_test.rs");
        }
    }
}
