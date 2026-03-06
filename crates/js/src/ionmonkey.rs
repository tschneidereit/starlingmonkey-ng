// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! JIT compiler options.
//!
//! This module provides access to SpiderMonkey's JIT compiler configuration,
//! allowing embedders to control IonMonkey and Baseline compiler behavior.

use crate::gc::scope::Scope;
use mozjs::jsapi::JSJitCompilerOption;
use mozjs::rust::wrappers2;

/// Set a global JIT compiler option.
pub fn set_option(scope: &Scope<'_>, opt: JSJitCompilerOption, value: u32) {
    unsafe { wrappers2::JS_SetGlobalJitCompilerOption(scope.cx(), opt, value) }
}

/// Get a global JIT compiler option.
///
/// Returns `None` if the query fails.
pub fn get_option(scope: &Scope<'_>, opt: JSJitCompilerOption) -> Option<u32> {
    let mut value: u32 = 0;
    let ok = unsafe { wrappers2::JS_GetGlobalJitCompilerOption(scope.cx(), opt, &mut value) };
    if ok {
        Some(value)
    } else {
        None
    }
}

/// Enable or disable off-thread baseline compilation.
pub fn set_offthread_baseline_compilation(scope: &Scope<'_>, enabled: bool) {
    unsafe { wrappers2::JS_SetOffthreadBaselineCompilationEnabled(scope.cx(), enabled) }
}

/// Enable or disable off-thread Ion compilation.
pub fn set_offthread_ion_compilation(scope: &Scope<'_>, enabled: bool) {
    unsafe { wrappers2::JS_SetOffthreadIonCompilationEnabled(scope.cx(), enabled) }
}
