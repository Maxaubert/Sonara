//! The Windows seams that touch nothing of the user's: the bounded wait on
//! a WinRT call (#16) and the session-end window (#18). Not `#[ignore]`:
//! they use a call and a window of their own.
#![cfg(windows)]

use sonara_system::platform::timed_out;
use sonara_system::win;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_ENDSESSION, WM_QUERYENDSESSION};
use windows_future::IAsyncOperation;

#[test]
fn a_winrt_call_that_never_answers_times_out() {
    let op = IAsyncOperation::<bool>::spawn(|| {
        std::thread::sleep(Duration::from_secs(3));
        Ok(true)
    });
    let started = Instant::now();
    let r = win::wait(op, Duration::from_millis(200));
    assert!(r.as_ref().is_err_and(|e| timed_out(e)), "{r:?}");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn a_winrt_call_that_answers_in_time_gives_its_result() {
    let op = IAsyncOperation::<bool>::spawn(|| Ok(true));
    assert_eq!(win::wait(op, Duration::from_secs(2)), Ok(true));
}

#[test]
fn the_session_end_window_runs_the_callback_only_when_the_session_ends() {
    let hits = Arc::new(AtomicU32::new(0));
    let h = hits.clone();
    let hwnd = win::watch_session_end(Box::new(move || {
        h.fetch_add(1, Ordering::SeqCst);
    }))
    .expect("the hidden window");
    let hwnd = HWND(hwnd as *mut _);
    // SAFETY: messages to this test's own window; SendMessage returns once
    // the window's thread handled them.
    unsafe {
        let r = SendMessageW(hwnd, WM_QUERYENDSESSION, Some(WPARAM(0)), Some(LPARAM(0)));
        assert_eq!(r.0, 1, "never holds up the logoff");
        // wParam 0: the session end was cancelled.
        SendMessageW(hwnd, WM_ENDSESSION, Some(WPARAM(0)), Some(LPARAM(0)));
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        SendMessageW(hwnd, WM_ENDSESSION, Some(WPARAM(1)), Some(LPARAM(0)));
    }
    assert_eq!(hits.load(Ordering::SeqCst), 1, "done before it returned");
}
