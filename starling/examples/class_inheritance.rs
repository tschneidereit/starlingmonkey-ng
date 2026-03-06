// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Example demonstrating class inheritance with prototype chain support.
//!
//! Defines Animal → Dog → Puppy hierarchy, testing:
//! - Prototype chain: parent methods callable on child instances
//! - `instanceof` checks across the hierarchy
//! - Stack-newtype upcast/downcast from Rust
//! - Constructor with parent initialization

use std::ptr;

use js::compile::evaluate_with_filename;
use js::gc::scope::Scope;
use js::native::Value;
use js::string as jsstring;
use libstarling::class::StackNewtype;
use libstarling::config::RuntimeConfig;
use libstarling::runtime::Runtime;
use libstarling::{jsclass, jsmethods};

#[jsclass]
struct Animal {
    name: String,
}

#[jsmethods]
impl Animal {
    #[constructor]
    fn new(name: String) -> Self {
        Self { name }
    }

    #[method]
    fn sound(&self) -> String {
        "...".to_string()
    }

    #[method]
    fn describe(&self) -> String {
        format!("Animal({})", self.name)
    }

    #[getter]
    fn name(&self) -> String {
        self.name.clone()
    }
}

#[jsclass(extends = Animal)]
struct Dog {
    parent: Animal,
    breed: String,
}

#[jsmethods]
impl Dog {
    #[constructor]
    fn new(name: String, breed: String) -> Self {
        Self {
            parent: __AnimalInner::new(name),
            breed,
        }
    }

    /// Override sound() from Animal
    #[method]
    fn sound(&self) -> String {
        "Woof!".to_string()
    }

    #[method]
    fn fetch(&self) -> String {
        format!("{} fetches the ball!", self.parent.name)
    }

    #[getter]
    fn breed(&self) -> String {
        self.breed.clone()
    }
}

#[jsclass(extends = Dog)]
struct Puppy {
    parent: Dog,
    age_months: i32,
}

#[jsmethods]
impl Puppy {
    #[constructor]
    fn new(name: String, breed: String, age_months: i32) -> Self {
        Self {
            parent: __DogInner::new(name, breed),
            age_months,
        }
    }

    #[getter]
    fn age_months(&self) -> i32 {
        self.age_months
    }

    #[method]
    fn describe(&self) -> String {
        format!(
            "Puppy({}, {}, {} months)",
            self.parent.parent.name, self.parent.breed, self.age_months
        )
    }
}

fn main() {
    let rt = Runtime::init(&RuntimeConfig::default());
    let scope = rt.default_global();
    let global = scope.global();

    // Register classes in parent-first order
    Animal::add_to_global(&scope, global);
    Dog::add_to_global(&scope, global);
    Puppy::add_to_global(&scope, global);

    // ====================================================================
    // Test 1: Basic construction and own methods
    // ====================================================================
    println!("Test 1: Basic construction");
    let rval = evaluate_with_filename(
        &scope,
        r#"
const animal = new Animal("Cat");
const dog = new Dog("Rex", "Labrador");
const puppy = new Puppy("Buddy", "Golden", 6);
animal.describe() + " | " + dog.fetch() + " | " + puppy.describe()
"#,
        "test1.js",
        1,
    )
    .expect("Test 1 script failed");
    let s = val_to_string(&scope, &rval);
    println!("  Result: {}", s);
    assert_eq!(
        s,
        "Animal(Cat) | Rex fetches the ball! | Puppy(Buddy, Golden, 6 months)"
    );

    // ====================================================================
    // Test 2: Inherited methods via prototype chain
    // ====================================================================
    println!("Test 2: Inherited methods");
    let rval =
        evaluate_with_filename(&scope, "dog.sound()", "test2.js", 1).expect("Test 2 script failed");
    let s = val_to_string(&scope, &rval);
    println!("  dog.sound() = {}", s);
    assert_eq!(s, "Woof!");

    // ====================================================================
    // Test 3: Inherited method from grandparent
    // ====================================================================
    println!("Test 3: Grandparent method on Puppy");
    let rval = evaluate_with_filename(
        &scope,
        r#"puppy.sound() + " | " + puppy.fetch()"#,
        "test3.js",
        1,
    )
    .expect("Test 3 script failed");
    let s = val_to_string(&scope, &rval);
    println!("  puppy.sound() + fetch(): {}", s);
    assert_eq!(s, "Woof! | Buddy fetches the ball!");

    // ====================================================================
    // Test 4: instanceof checks
    // ====================================================================
    println!("Test 4: instanceof");
    let rval = evaluate_with_filename(
        &scope,
        r#"
var results = [];
results.push("dog instanceof Dog: " + (dog instanceof Dog));
results.push("dog instanceof Animal: " + (dog instanceof Animal));
results.push("puppy instanceof Puppy: " + (puppy instanceof Puppy));
results.push("puppy instanceof Dog: " + (puppy instanceof Dog));
results.push("puppy instanceof Animal: " + (puppy instanceof Animal));
results.push("animal instanceof Dog: " + (animal instanceof Dog));
results.join(", ")
"#,
        "test4.js",
        1,
    )
    .expect("Test 4 script failed");
    let s = val_to_string(&scope, &rval);
    println!("  {}", s);
    assert!(s.contains("dog instanceof Dog: true"));
    assert!(s.contains("dog instanceof Animal: true"));
    assert!(s.contains("puppy instanceof Puppy: true"));
    assert!(s.contains("puppy instanceof Dog: true"));
    assert!(s.contains("puppy instanceof Animal: true"));
    assert!(s.contains("animal instanceof Dog: false"));

    // ====================================================================
    // Test 5: Prototype chain structure
    // ====================================================================
    println!("Test 5: Prototype chain structure");
    let rval = evaluate_with_filename(
        &scope,
        r#"
var r = [];
r.push("Dog proto -> Animal proto: " +
    (Object.getPrototypeOf(Dog.prototype) === Animal.prototype));
r.push("Puppy proto -> Dog proto: " +
    (Object.getPrototypeOf(Puppy.prototype) === Dog.prototype));
r.join(", ")
"#,
        "test5.js",
        1,
    )
    .expect("Test 5 script failed");
    let s = val_to_string(&scope, &rval);
    println!("  {}", s);
    assert!(s.contains("Dog proto -> Animal proto: true"));
    assert!(s.contains("Puppy proto -> Dog proto: true"));

    // ====================================================================
    // Test 6: Rust-side stack newtype upcast
    // ====================================================================
    println!("Test 6: Upcast");
    let dog = Dog::new(&scope, "Fido".to_string(), "Poodle".to_string());
    let animal: Animal = dog.upcast();
    let animal_name = animal.name();
    assert_eq!(animal_name, "Fido");
    println!("  Upcast Dog->Animal name: {}", animal_name);

    // ====================================================================
    // Test 7: Rust-side stack newtype downcast
    // ====================================================================
    println!("Test 7: Downcast");
    // Downcast from Animal that's actually a Dog
    let dog_back: Dog = animal
        .cast::<Dog>()
        .expect("Expected successful downcast to Dog");
    let dog_breed = dog_back.breed();
    assert_eq!(dog_breed, "Poodle");
    println!("  Downcast Animal->Dog breed: {}", dog_breed);

    // Downcast to wrong type should fail
    let bad_down: Option<Puppy> = animal.cast::<Puppy>();
    assert!(bad_down.is_none(), "Expected failed downcast to Puppy");
    println!("  Downcast Animal->Puppy: None (correct)");

    // ====================================================================
    // Test 8: Multi-level upcast/downcast
    // ====================================================================
    println!("Test 8: Multi-level upcast/downcast");
    let puppy = Puppy::new(&scope, "Tiny".to_string(), "Chihuahua".to_string(), 3);
    // Upcast to Dog
    let as_dog: Dog = puppy.upcast();
    let dog_breed = as_dog.breed();
    assert_eq!(dog_breed, "Chihuahua");
    println!("  Puppy->Dog breed: {}", dog_breed);

    // Upcast to Animal (chain: Puppy -> Dog -> Animal)
    let as_animal: Animal = as_dog.upcast();
    let animal_name = as_animal.name();
    assert_eq!(animal_name, "Tiny");
    println!("  Puppy->Dog->Animal name: {}", animal_name);

    // Downcast back from Animal to Puppy
    let puppy_back: Puppy = as_animal
        .cast::<Puppy>()
        .expect("Expected successful downcast to Puppy");
    let puppy_age = puppy_back.age_months();
    assert_eq!(puppy_age, 3);
    println!("  Animal->Puppy age: {}", puppy_age);

    println!("All tests passed!");
}

/// Helper: extract a Rust String from a JS string value.
fn val_to_string(scope: &Scope<'_>, val: &Value) -> String {
    assert!(val.is_string(), "Expected string value");
    let str_handle = scope.root_string(ptr::NonNull::new(val.to_string()).expect("null string"));
    jsstring::to_utf8(scope, str_handle).expect("utf8 conversion failed")
}

#[test]
fn class_inheritance_example() {
    main()
}
