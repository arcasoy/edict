pub struct Invoice;

pub(super) mod lines {} // fine: visible in `billing` only
pub(in crate::billing) mod detail {} // fine: scoped to `billing`
pub(in crate) mod deep {} // leak: scoped to the whole crate

#[cfg(test)]
pub(crate) mod tests {} // ignored: test-only
