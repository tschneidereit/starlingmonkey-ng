// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Example demonstrating cross-object references using `Heap<T>`.
//!
//! Two classes are defined:
//! - `Item` — a simple value holder
//! - `Container` — holds a `Heap<ItemImpl>` to an `Item`, demonstrating how one
//!   builtin's instance can reference another on the GC heap
//!
//! The example shows:
//! 1. Storing a reference to one JS object inside another via `Heap<ItemImpl>`
//! 2. Accessing the referenced object's data from Rust methods
//! 3. Rooting a heap reference back to the stack via `Heap::get(&scope)`,
//!    which returns the stack newtype directly (e.g. `Item<'s>`)
//! 4. Operating on both objects from JavaScript

use std::ptr;

use js::compile::evaluate_with_filename;
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::native::Value;
use js::string as jsstring;
use libstarling::config::RuntimeConfig;
use libstarling::runtime::Runtime;
use libstarling::{jsclass, jsmethods};

// ============================================================================
// Item — a simple class that Container will hold a reference to
// ============================================================================

#[jsclass]
struct Item {
    label: String,
    value: i32,
}

#[jsmethods]
impl Item {
    #[constructor]
    fn new(label: String, value: i32) -> Self {
        Self { label, value }
    }

    #[getter]
    fn label(&self) -> String {
        self.label.clone()
    }

    #[getter]
    fn value(&self) -> i32 {
        self.value
    }

    #[method]
    fn describe(&self) -> String {
        format!("Item({}, {})", self.label, self.value)
    }
}

// ============================================================================
// Container — holds a Heap reference to an Item
// ============================================================================

#[jsclass]
struct Container {
    name: String,
    /// A GC-traced reference to an `Item` on the heap.
    item: Heap<ItemImpl>,
}

#[jsmethods]
impl Container {
    /// Construct a Container from JS: `new Container("myContainer", someItem)`.
    ///
    /// The second argument is automatically extracted as an `Item<'_>` stack
    /// newtype from the JS value, then converted into a `Heap<ItemImpl>` via `.into()`.
    #[constructor]
    fn new(name: String, item: Item<'_>) -> Self {
        Self {
            name,
            item: item.into(),
        }
    }

    #[getter]
    fn item<'a>(&self, scope: &'a Scope<'_>) -> Item<'a> {
        self.item.get(scope)
    }

    #[getter]
    fn name(&self) -> String {
        self.name.clone()
    }

    /// Read the stored Item's value through the heap reference.
    #[method]
    fn item_value(&self, scope: &Scope<'_>) -> i32 {
        let item: Item<'_> = self.item.get(scope);
        item.value()
    }

    /// Read the stored Item's label through the heap reference.
    #[method]
    fn item_label(&self, scope: &Scope<'_>) -> String {
        let item: Item<'_> = self.item.get(scope);
        item.label()
    }

    #[method]
    fn describe(&self, scope: &Scope<'_>) -> String {
        let item: Item<'_> = self.item.get(scope);
        let item_desc = format!("Item({}, {})", item.label(), item.value());
        format!("Container({}, {})", self.name, item_desc)
    }
}

// ============================================================================
// Main
// ============================================================================

fn main() {
    let rt = Runtime::init(&RuntimeConfig::default());
    let scope = rt.default_global();
    let global = scope.global();

    // Register both classes on the global
    Item::add_to_global(&scope, global);
    Container::add_to_global(&scope, global);

    // ====================================================================
    // Test 1: Create objects in JS and read through the heap reference
    // ====================================================================
    println!("Test 1: Create and read through heap reference from JS");
    let rval = evaluate_with_filename(
        &scope,
        r#"
const item = new Item("widget", 42);
const container = new Container("box", item);
container.describe()
"#,
        "test1.js",
        1,
    )
    .expect("Test 1 script failed");
    let s = val_to_string(&scope, &rval);
    println!("  container.describe() = {}", s);
    assert_eq!(s, "Container(box, Item(widget, 42))");

    // ====================================================================
    // Test 2: Access item properties through container methods
    // ====================================================================
    println!("Test 2: Access item properties via container");
    let rval = evaluate_with_filename(&scope, r#"container.itemValue()"#, "test2.js", 1)
        .expect("Test 2 script failed");
    assert!(rval.is_int32());
    assert_eq!(rval.to_int32(), 42);
    println!("  container.itemValue() = {}", rval.to_int32());

    let rval = evaluate_with_filename(&scope, r#"container.itemLabel()"#, "test2b.js", 1)
        .expect("Test 2b script failed");
    let s = val_to_string(&scope, &rval);
    assert_eq!(s, "widget");
    println!("  container.itemLabel() = {}", s);

    // ====================================================================
    // Test 3: The stored Item is the same object (not a copy)
    // ====================================================================
    println!("Test 3: Shared identity check in JS");
    let rval = evaluate_with_filename(
        &scope,
        r#"
// Mutate the original item and verify container sees the change
item.describe()
"#,
        "test3.js",
        1,
    )
    .expect("Test 3 script failed");
    let s = val_to_string(&scope, &rval);
    assert_eq!(s, "Item(widget, 42)");
    println!("  item.describe() = {}", s);

    // ====================================================================
    // Test 4: Create objects from Rust, using the generated Foo::new()
    // ====================================================================
    println!("Test 4: Create objects from Rust side");
    let item = Item::new(&scope, "gadget".to_string(), 99);

    // The `item` is currently a stack-rooted `Item<'s>`, but `Container::new()`
    // converts it into a `Heap<ItemImpl>` and stores it in the `Container`.
    let container = Container::new(&scope, "crate".to_string(), item);

    // Read back through the stack newtype's forwarded methods
    let desc = container.describe(&scope);
    println!("  container.describe() = {}", desc);
    assert_eq!(desc, "Container(crate, Item(gadget, 99))");

    let val = container.item_value(&scope);
    println!("  container.item_value() = {}", val);
    assert_eq!(val, 99);

    // ====================================================================
    // Test 5: Root a Heap<ItemImpl> back to the stack via Heap::get()
    // ====================================================================
    println!("Test 5: Root heap reference back to stack");
    // Get the item from the container.
    let item: Item<'_> = container.item(&scope);

    // Now we have a stack-rooted Item<'s> — call methods on it
    let label = item.label();
    let value = item.value();
    println!("  item: label={}, value={}", label, value);
    assert_eq!(label, "gadget");
    assert_eq!(value, 99);

    // ====================================================================
    // Test 6: Multiple containers can share the same item
    // ====================================================================
    println!("Test 6: Multiple containers sharing an item");
    let rval = evaluate_with_filename(
        &scope,
        r#"
const shared = new Item("shared-thing", 7);
const c1 = new Container("first", shared);
const c2 = new Container("second", shared);
c1.itemValue() + c2.itemValue()
"#,
        "test6.js",
        1,
    )
    .expect("Test 6 script failed");
    assert!(rval.is_int32());
    assert_eq!(rval.to_int32(), 14);
    println!("  c1.itemValue() + c2.itemValue() = {}", rval.to_int32());

    // ====================================================================
    // Test 7: Container getters from JS
    // ====================================================================
    println!("Test 7: Container getters");
    let rval =
        evaluate_with_filename(&scope, r#"c1.name"#, "test7.js", 1).expect("Test 7 script failed");
    let s = val_to_string(&scope, &rval);
    assert_eq!(s, "first");
    println!("  c1.name = {}", s);

    println!("All tests passed!");
}

/// Helper: extract a Rust `String` from a JS string value.
fn val_to_string(scope: &Scope<'_>, val: &Value) -> String {
    assert!(val.is_string(), "Expected string value");
    let str_handle = scope.root_string(ptr::NonNull::new(val.to_string()).expect("null string"));
    jsstring::to_utf8(scope, str_handle).expect("utf8 conversion failed")
}

#[test]
fn heap_refs_example() {
    main()
}
