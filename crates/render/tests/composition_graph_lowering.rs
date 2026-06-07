//! Integration tests for `CompositionGraph` node lowering.
//!
//! These tests pin the FFmpeg-filter strings emitted by
//! `montage_render::professional::lower_composition_graph` so that the
//! `CompositionGraph` runtime actually drives the render pipeline rather than
//! emitting placeholder identifiers. Coverage is currently MVP-scoped to the
//! Blur and Mask node types (see overnight-wave1-vfx-graph).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use montage_proto::professional::{
    CompositionEdge, CompositionGraph, CompositionNode, CompositionNodeType,
};
use montage_render::professional::{RenderLoweringStep, lower_composition_graph};
use serde_json::{Value, json};
use std::collections::HashMap;

fn params<const N: usize>(pairs: [(&str, Value); N]) -> HashMap<String, Value> {
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

fn node(id: &str, node_type: CompositionNodeType, p: HashMap<String, Value>) -> CompositionNode {
    CompositionNode {
        id: id.into(),
        node_type,
        params: p,
    }
}

fn edge(from: &str, to: &str) -> CompositionEdge {
    CompositionEdge {
        from: from.into(),
        to: to.into(),
        input: None,
    }
}

fn find_step<'a>(steps: &'a [RenderLoweringStep], id: &str) -> &'a RenderLoweringStep {
    steps
        .iter()
        .find(|step| step.node_id == id)
        .unwrap_or_else(|| panic!("expected lowering step for node {id}"))
}

#[test]
fn blur_node_with_box_method_lowers_to_boxblur_filter() {
    let graph = CompositionGraph {
        id: "blur-box".into(),
        nodes: vec![
            node(
                "blur",
                CompositionNodeType::Blur,
                params([("method", json!("box")), ("radius", json!(8.0))]),
            ),
            node(
                "output",
                CompositionNodeType::Output,
                HashMap::<String, Value>::new(),
            ),
        ],
        edges: vec![edge("blur", "output")],
        output_node_id: Some("output".into()),
    };

    let lowering = lower_composition_graph(&graph);
    let step = find_step(&lowering.steps, "blur");

    assert_eq!(step.backend, "ffmpeg");
    assert!(
        step.expression.contains("boxblur="),
        "expected boxblur filter, got: {}",
        step.expression
    );
    assert!(
        step.expression.contains("luma_radius=8"),
        "expected radius=8 propagated to boxblur, got: {}",
        step.expression
    );
}

#[test]
fn blur_node_with_gaussian_method_lowers_to_gblur_filter() {
    let graph = CompositionGraph {
        id: "blur-gauss".into(),
        nodes: vec![
            node(
                "blur",
                CompositionNodeType::Blur,
                params([("method", json!("gaussian")), ("radius", json!(4.0))]),
            ),
            node(
                "output",
                CompositionNodeType::Output,
                HashMap::<String, Value>::new(),
            ),
        ],
        edges: vec![edge("blur", "output")],
        output_node_id: Some("output".into()),
    };

    let lowering = lower_composition_graph(&graph);
    let step = find_step(&lowering.steps, "blur");

    assert!(
        step.expression.contains("gblur=") && step.expression.contains("sigma=4"),
        "expected gblur with sigma=4, got: {}",
        step.expression
    );
}

#[test]
fn blur_node_defaults_to_box_method_when_unspecified() {
    let graph = CompositionGraph {
        id: "blur-default".into(),
        nodes: vec![
            node(
                "blur",
                CompositionNodeType::Blur,
                params([("radius", json!(2.0))]),
            ),
            node(
                "output",
                CompositionNodeType::Output,
                HashMap::<String, Value>::new(),
            ),
        ],
        edges: vec![edge("blur", "output")],
        output_node_id: Some("output".into()),
    };

    let lowering = lower_composition_graph(&graph);
    let step = find_step(&lowering.steps, "blur");

    assert!(
        step.expression.starts_with("boxblur="),
        "expected default box blur, got: {}",
        step.expression
    );
    assert!(
        step.expression.contains("luma_radius=2"),
        "expected radius=2, got: {}",
        step.expression
    );
}

#[test]
fn blur_node_clamps_negative_radius_to_zero() {
    let graph = CompositionGraph {
        id: "blur-neg".into(),
        nodes: vec![
            node(
                "blur",
                CompositionNodeType::Blur,
                params([("method", json!("box")), ("radius", json!(-3.0))]),
            ),
            node(
                "output",
                CompositionNodeType::Output,
                HashMap::<String, Value>::new(),
            ),
        ],
        edges: vec![edge("blur", "output")],
        output_node_id: Some("output".into()),
    };

    let lowering = lower_composition_graph(&graph);
    let step = find_step(&lowering.steps, "blur");

    assert!(
        step.expression.contains("luma_radius=0"),
        "expected clamped radius=0, got: {}",
        step.expression
    );
}

#[test]
fn mask_node_rect_kind_emits_geq_and_alphamerge_chain() {
    let graph = CompositionGraph {
        id: "mask-rect".into(),
        nodes: vec![
            node(
                "mask",
                CompositionNodeType::Mask,
                params([
                    ("kind", json!("rect")),
                    ("x", json!(0.1)),
                    ("y", json!(0.2)),
                    ("width", json!(0.5)),
                    ("height", json!(0.4)),
                ]),
            ),
            node(
                "output",
                CompositionNodeType::Output,
                HashMap::<String, Value>::new(),
            ),
        ],
        edges: vec![edge("mask", "output")],
        output_node_id: Some("output".into()),
    };

    let lowering = lower_composition_graph(&graph);
    let step = find_step(&lowering.steps, "mask");

    assert_eq!(step.backend, "ffmpeg");
    assert!(
        step.expression.contains("geq="),
        "expected geq-synthesized alpha mask for rect kind, got: {}",
        step.expression
    );
    assert!(
        step.expression.contains("alphamerge"),
        "expected alphamerge to apply the synthesized mask, got: {}",
        step.expression
    );
    // Normalized rect origin coords must be wired into the alpha expression
    // so the synthesized mask actually crops the configured region.
    assert!(
        step.expression.contains("0.1") && step.expression.contains("0.2"),
        "expected mask rect origin coordinates in expression, got: {}",
        step.expression
    );
}

#[test]
fn mask_node_ellipse_kind_emits_ellipse_alpha_expression() {
    let graph = CompositionGraph {
        id: "mask-ellipse".into(),
        nodes: vec![
            node(
                "mask",
                CompositionNodeType::Mask,
                params([
                    ("kind", json!("ellipse")),
                    ("x", json!(0.0)),
                    ("y", json!(0.0)),
                    ("width", json!(1.0)),
                    ("height", json!(1.0)),
                ]),
            ),
            node(
                "output",
                CompositionNodeType::Output,
                HashMap::<String, Value>::new(),
            ),
        ],
        edges: vec![edge("mask", "output")],
        output_node_id: Some("output".into()),
    };

    let lowering = lower_composition_graph(&graph);
    let step = find_step(&lowering.steps, "mask");

    // Ellipse masks must use a non-rect alpha expression so the resulting
    // mask is rounded rather than a hard rectangle.
    assert!(
        step.expression.contains("ellipse")
            || step.expression.contains("pow(")
            || step.expression.contains("hypot"),
        "expected ellipse alpha expression, got: {}",
        step.expression
    );
    assert!(
        step.expression.contains("alphamerge"),
        "expected alphamerge in ellipse mask chain, got: {}",
        step.expression
    );
}

#[test]
fn mask_node_invert_flag_negates_alpha() {
    let graph = CompositionGraph {
        id: "mask-invert".into(),
        nodes: vec![
            node(
                "mask",
                CompositionNodeType::Mask,
                params([
                    ("kind", json!("rect")),
                    ("invert", json!(true)),
                    ("x", json!(0.0)),
                    ("y", json!(0.0)),
                    ("width", json!(0.5)),
                    ("height", json!(0.5)),
                ]),
            ),
            node(
                "output",
                CompositionNodeType::Output,
                HashMap::<String, Value>::new(),
            ),
        ],
        edges: vec![edge("mask", "output")],
        output_node_id: Some("output".into()),
    };

    let lowering = lower_composition_graph(&graph);
    let step = find_step(&lowering.steps, "mask");

    // Inversion must be visible in the emitted filter chain.
    assert!(
        step.expression.contains("negate") || step.expression.contains("lutrgb"),
        "expected negate/lutrgb to invert mask, got: {}",
        step.expression
    );
}

#[test]
fn mask_node_alpha_in_kind_uses_alphamerge_without_geq() {
    let graph = CompositionGraph {
        id: "mask-alpha-in".into(),
        nodes: vec![
            node(
                "mask",
                CompositionNodeType::Mask,
                params([("kind", json!("alpha_in"))]),
            ),
            node(
                "output",
                CompositionNodeType::Output,
                HashMap::<String, Value>::new(),
            ),
        ],
        edges: vec![edge("mask", "output")],
        output_node_id: Some("output".into()),
    };

    let lowering = lower_composition_graph(&graph);
    let step = find_step(&lowering.steps, "mask");

    // alpha_in passes the upstream alpha straight through alphamerge with no
    // synthesized geq stage.
    assert!(
        step.expression.contains("alphamerge"),
        "expected alphamerge for alpha_in, got: {}",
        step.expression
    );
    assert!(
        !step.expression.contains("geq="),
        "alpha_in should not synthesize a geq mask, got: {}",
        step.expression
    );
}
