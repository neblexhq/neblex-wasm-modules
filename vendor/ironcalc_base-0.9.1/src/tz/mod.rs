#[cfg(ironcalc_browser)]
mod browser_tz;
#[cfg(ironcalc_browser)]
pub(crate) use browser_tz::*;

#[cfg(not(ironcalc_browser))]
mod os_tz;
#[cfg(not(ironcalc_browser))]
pub(crate) use os_tz::*;
