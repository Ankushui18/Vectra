//! **Shader law**: the WGSL that ships is valid, and it agrees with the Rust
//! that feeds it (Task 5.0 §1/§5).
//!
//! There is no GPU in CI, so the shader is tested the way the compiler tests it:
//! `naga` — the front end wgpu itself uses — parses and type-checks the module.
//! That catches the failures a `wgpu` pipeline would raise at runtime: a syntax
//! error, an undeclared binding, a type mismatch between the vertex stage's
//! outputs and the fragment stage's inputs.
//!
//! On top of *validity*, these laws assert the **coupling**: the bindings the
//! shader declares are exactly the ones `GpuRenderer::new` creates, and the
//! `Instance` struct's rows are exactly the fields `InstanceRaw` packs. A
//! mismatch there is invisible until something is drawn wrongly, which is
//! precisely the class of bug a shader test should own.

use naga::AddressSpace;
use vectra_render::gpu::{
    FILL_BLEND_ENTRY, FILL_ENTRY, SHADER_SOURCE, STROKE_BLEND_ENTRY, STROKE_ENTRY, VERTEX_ENTRY,
};
use vectra_render::InstanceRaw;

fn module() -> naga::Module {
    naga::front::wgsl::parse_str(SHADER_SOURCE).expect("shader.wgsl parses as WGSL")
}

fn validate(module: &naga::Module) -> naga::valid::ModuleInfo {
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    );
    validator
        .validate(module)
        .expect("shader.wgsl passes naga validation")
}

#[test]
fn the_shader_parses_and_validates() {
    let module = module();
    validate(&module);
}

#[test]
fn the_shader_declares_the_entry_points_the_pipelines_ask_for() {
    let module = module();
    let names: Vec<&str> = module
        .entry_points
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert!(names.contains(&VERTEX_ENTRY), "{names:?}");
    assert!(names.contains(&FILL_ENTRY), "{names:?}");
    assert!(names.contains(&STROKE_ENTRY), "{names:?}");
    // The backdrop pair (Task 10.2 RULE 3): `Multiply`, `Screen` and `Overlay`
    // are functions of the destination, so they cannot be a blend state — they
    // are these two entry points, bound to the pipelines that use `REPLACE`.
    assert!(names.contains(&FILL_BLEND_ENTRY), "{names:?}");
    assert!(names.contains(&STROKE_BLEND_ENTRY), "{names:?}");
    // Exactly one vertex stage: both pipelines share it, which is what makes
    // "same geometry, different colour row" a fragment-stage concern.
    let vertex_stages = module
        .entry_points
        .iter()
        .filter(|entry| entry.stage == naga::ShaderStage::Vertex)
        .count();
    assert_eq!(vertex_stages, 1);
}

#[test]
fn the_bindings_match_what_the_rust_bind_group_layout_declares() {
    let module = module();
    let mut bindings: Vec<(u32, u32, String, AddressSpace)> = module
        .global_variables
        .iter()
        .filter_map(|(_, variable)| {
            let binding = variable.binding.as_ref()?;
            Some((
                binding.group,
                binding.binding,
                variable.name.clone().unwrap_or_default(),
                variable.space,
            ))
        })
        .collect();
    bindings.sort_by_key(|(group, binding, _, _)| (*group, *binding));
    assert_eq!(bindings.len(), 4, "camera + instances + ramps + backdrop");

    let (group, binding, name, space) = &bindings[0];
    assert_eq!((*group, *binding, name.as_str()), (0, 0, "camera"));
    assert_eq!(
        *space,
        AddressSpace::Uniform,
        "binding 0 is the camera uniform"
    );

    let (group, binding, name, space) = &bindings[1];
    assert_eq!((*group, *binding, name.as_str()), (0, 1, "instances"));
    match space {
        // Read-only storage is what lets the instance array be sized by the
        // scene instead of a compile-time maximum.
        AddressSpace::Storage { access } => {
            assert_eq!(*access, naga::StorageAccess::LOAD, "read-only");
        }
        other => panic!("instances must be storage, got {other:?}"),
    }

    // The gradient ramp table: read-only storage, and reachable from the
    // **fragment** stage, which is where a ramp is sampled.
    let (group, binding, name, space) = &bindings[2];
    assert_eq!((*group, *binding, name.as_str()), (0, 2, "ramps"));
    assert!(matches!(space, AddressSpace::Storage { .. }));

    // The backdrop snapshot lives in its own group, because its *view* changes
    // when the target is resized while the camera/instance group does not.
    let (group, binding, name, space) = &bindings[3];
    assert_eq!((*group, *binding, name.as_str()), (1, 0, "backdrop"));
    assert_eq!(*space, AddressSpace::Handle);
}

#[test]
fn the_instance_struct_has_the_rows_rust_packs() {
    let module = module();
    let instance = module
        .types
        .iter()
        .find(|(_, ty)| ty.name.as_deref() == Some("Instance"))
        .map(|(_, ty)| ty)
        .expect("the shader declares struct Instance");
    let naga::TypeInner::Struct { members, span } = &instance.inner else {
        panic!("Instance is a struct");
    };
    let names: Vec<&str> = members
        .iter()
        .map(|member| member.name.as_deref().unwrap_or(""))
        .collect();
    // Task 10.2 RULE 3: the row carries the item's colour, its blend code, and the
    // window into the ramp table — one row per **draw item**, not per node.
    assert_eq!(names, ["transform", "color", "params", "ramp", "frame"]);
    for member in members {
        match &module.types[member.ty].inner {
            naga::TypeInner::Vector { size, scalar } => {
                assert_eq!(*size, naga::VectorSize::Quad);
                assert_eq!(
                    *scalar,
                    naga::Scalar {
                        kind: naga::ScalarKind::Float,
                        width: 4
                    }
                );
            }
            other => panic!("Instance rows must be vec4<f32>, got {other:?}"),
        }
    }
    // And the whole struct is exactly the stride the buffer planner uses.
    assert_eq!(*span as u64, InstanceRaw::STRIDE, "struct span == STRIDE");
}

#[test]
fn the_camera_struct_matches_the_camera_uniform() {
    let module = module();
    let camera = module
        .types
        .iter()
        .find(|(_, ty)| ty.name.as_deref() == Some("Camera"))
        .map(|(_, ty)| ty)
        .expect("the shader declares struct Camera");
    let naga::TypeInner::Struct { members, span } = &camera.inner else {
        panic!("Camera is a struct");
    };
    assert_eq!(members.len(), 2);
    assert_eq!(*span as u64, 32, "two vec4 rows");
}

#[test]
fn the_vertex_stage_reads_the_vertex_layout_and_the_instance_index() {
    // `vertex_layout()` binds a single `Float32x2` at location 0, and the draw
    // ranges the instance slot as `@builtin(instance_index)`. Both have to be
    // consumed exactly as declared, or every vertex reads the wrong node's data.
    let module = module();
    let vertex = module
        .entry_points
        .iter()
        .find(|entry| entry.name == VERTEX_ENTRY)
        .expect("vs_main");

    let input = vertex
        .function
        .arguments
        .first()
        .expect("vs_main takes the vertex input struct");
    let naga::TypeInner::Struct { members, .. } = &module.types[input.ty].inner else {
        panic!("the first argument is a struct");
    };
    assert_eq!(members.len(), 1, "one vertex attribute");
    let member = &members[0];
    match &member.binding {
        Some(naga::Binding::Location { location, .. }) => assert_eq!(*location, 0),
        other => panic!("the position must be at location 0, got {other:?}"),
    }
    match &module.types[member.ty].inner {
        naga::TypeInner::Vector { size, scalar } => {
            assert_eq!(*size, naga::VectorSize::Bi, "vec2<f32>");
            assert_eq!(scalar.width, 4);
        }
        other => panic!("position must be vec2<f32>, got {other:?}"),
    }

    let has_instance_index = vertex.function.arguments.iter().any(|argument| {
        matches!(
            argument.binding,
            Some(naga::Binding::BuiltIn(naga::BuiltIn::InstanceIndex))
        )
    });
    assert!(
        has_instance_index,
        "the vertex stage must index the instance array by instance_index"
    );
}
