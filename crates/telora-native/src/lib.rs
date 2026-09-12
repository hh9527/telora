//! Independent native backend. No old VM, Val or Heap adapters.
pub mod abi;
#[cfg(feature = "jit")]
pub mod jit;
pub mod runtime;
#[cfg(test)]
mod test_support;
