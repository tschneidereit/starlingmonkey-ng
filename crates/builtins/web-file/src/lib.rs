use js::{gc::scope::Scope, Object};

mod blob;
mod file;

pub fn add_to_global(scope: &Scope<'_>, global: Object<'_>) {
    blob::Blob::add_to_global(scope, global);
    file::File::add_to_global(scope, global);
}
