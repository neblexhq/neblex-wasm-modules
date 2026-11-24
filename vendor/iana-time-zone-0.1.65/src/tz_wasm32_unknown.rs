// Patched for sandboxed modules: no host imports, the local time zone is always UTC.
pub(crate) fn get_timezone_inner() -> Result<String, crate::GetTimezoneError> {
    Ok("UTC".to_string())
}
