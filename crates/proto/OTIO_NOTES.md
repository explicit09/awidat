# OTIO superset notes

The OTIO scope here is wider than a "minimal viable" subset because
`apply_edl` and the TUI need typed access to markers and effects. Adding
them later as `serde_json::Value` blobs would calcify into a wrong shape.

## What we model

The Montage project format is **OpenTimelineIO 1.x JSON, plus a single
namespaced metadata key `montage`**. The Rust types in
[`src/otio/`](src/otio/) are a typed subset of OTIO 1.x.

| OTIO type | Rust type | Why we model it |
| --- | --- | --- |
| `Timeline.1` | [`Timeline`](src/otio/nodes.rs) | Root container. Mandatory. |
| `Stack.1` | [`Stack`](src/otio/nodes.rs) | Layered children (b-roll over interview). Stacks are also nestable (`StackChild::Stack`). |
| `Track.1` | [`Track`](src/otio/nodes.rs) | Sequential clips, video or audio. The day-1 unit of editing. |
| `Clip.1` | [`Clip`](src/otio/nodes.rs) | Single piece of media on a track. |
| `Gap.1` | [`Gap`](src/otio/nodes.rs) | Empty space — needed because trim semantics in `apply_edl` produce gaps explicitly. |
| `ExternalReference.1` | [`ExternalReference`](src/otio/nodes.rs) | Reference to a media file. The common case. |
| `MissingReference.1` | [`MissingReference`](src/otio/nodes.rs) | Used when the agent says "I want a b-roll insert here but the asset isn't loaded yet". Default `media_reference` of a fresh `Clip`. |
| `Effect.1` | [`Effect`](src/otio/nodes.rs) | Base effect type. v1 holds `effect_name: String` + `metadata: serde_json::Value`. Specializations land in v1.5+. The typed *slot* (`Vec<Effect>` on `Clip`) is what we needed Week 1. |
| `Marker.1` | [`Marker`](src/otio/nodes.rs) | Timeline annotation. Heavy use by Week 4 `apply_edl` and the agent ("the laugh at 4:12", "speaker mentioned Stripe here"). |
| `RationalTime` | [`RationalTime`](src/otio/time.rs) | Value type for time. `value: f64`, `rate: f64`. Matches the OTIO spec. |
| `TimeRange` | [`TimeRange`](src/otio/time.rs) | Value type for spans. `start_time + duration`, `duration ≥ 0`. |

## What we deliberately skip

These are real OTIO 1.x types we are **not** modeling in v1:

| Skipped | Why |
| --- | --- |
| `SerializableCollection.1` | Project-of-projects. Wrong for a podcast-episode workflow. |
| `LinearTimeWarp.1`, `FreezeFrame.1`, time-effect specializations | Speed ramps and freeze frames are out of scope until Week 5+ creative effects. The base [`Effect`](src/otio/nodes.rs) handles their *slot*. |
| `GeneratorReference.1` | Procedural media (color bars, slugs). Not used by spoken-word workflows. |
| `ImageSequenceReference.1` | Image-sequence media. Not used by podcast/interview footage. |
| `SchemaDef.1` | Plug-in OTIO schemas. We're an end-user of OTIO, not a schema host. |

These are not "bad" types; they're just not on the v1 demo path. Adding
support is purely additive (see [Adding a new OTIO type](#adding-a-new-otio-type-worked-example)).

## Schema versioning rules

Every OTIO node carries an `OTIO_SCHEMA: "Name.Major"` discriminator.

1. **Known name + matching major** → parsed as the matching variant. The
   happy path. Example: `"Clip.1"` ✓ when we ship `Clip.1`.

2. **Known name + unknown major** → forward-compat. We rewrite the schema
   string to our supported major, deserialize as that variant, and surface
   a [`SchemaWarning`](src/otio/schema.rs). Example: `"Clip.2"` is read
   *as if* it were `"Clip.1"` and a warning is recorded. The `montage
   validate` CLI prints the warnings in a `Schema warnings:` section.

   Rationale: hand-edited files coming from a future Montage version, or
   from a third-party OTIO adapter that uses a slightly newer revision,
   should not bounce. They should load with a warning the user can act on.

3. **Unknown name** → hard fail. Example: `"Foo.1"` produces
   [`ProtoError::UnknownOtioSchema`](src/error.rs) listing every supported
   name.

   Rationale: we're parsing a *typed* subset. We literally have no
   variant to deserialize an unknown name into; silently dropping it would
   lose data.

4. **Malformed `OTIO_SCHEMA`** (not `Name.Major`) → hard fail with
   [`ProtoError::MalformedOtioSchema`](src/error.rs).

The supported-name list lives in
[`SUPPORTED_SCHEMA_NAMES`](src/otio/schema.rs). Adding a name there is
the only way to grow the typed surface — adding an enum variant without
touching the list is a bug.

### Where the schema rewriting happens

[`crate::project::read_otio_timeline`](src/project.rs) parses the file
into a `serde_json::Value`, walks every `OTIO_SCHEMA` field via
[`rewrite_schema_strings`](src/project.rs), records warnings, and rewrites
forward-compat schemas to the supported major. Then `serde_json::
from_value` runs the typed deserialize; by that point every
`OTIO_SCHEMA` matches a known variant exactly.

This is what lets the typed deserialize stay pure (no custom serde trait
impls per type for forward-compat).

## The `montage` metadata namespace

The only schema *extension* we make to OTIO is the `metadata.montage`
block. We model it strongly (NOT `serde_json::Value`)
so unknown fields surface as parse errors against our own schema.

Three locations:

| Location | Type | Purpose |
| --- | --- | --- |
| `Timeline.metadata.montage` | [`MontageTimelineMetadata`](src/montage_meta.rs) | Project-wide: source assets, anchor table, edit-plan reference. |
| `Clip.metadata.montage` | [`MontageClipMetadata`](src/montage_meta.rs) | Per-clip: agent's reasoning, edit-plan back-reference, optional inline anchor. |
| `Marker.metadata.montage` | [`MontageMarkerMetadata`](src/montage_meta.rs) | Per-marker: category (e.g. `"laugh"`, `"key-quote"`), free note. |

Other metadata namespaces (other tools writing under `metadata.<name>`)
are preserved verbatim via a `flatten`-ed `HashMap<String,
serde_json::Value>` field. Round-trip stability for foreign metadata
matters because OTIO is the lingua franca of pro NLEs and we don't want
to clobber Resolve's or Premiere's metadata when a user round-trips.

## Internal serialization: how we avoid duplicate `OTIO_SCHEMA`

Most OTIO node types appear inside `#[serde(tag = "OTIO_SCHEMA")]` enums
([`StackChild`](src/otio/nodes.rs), [`TrackChild`](src/otio/nodes.rs),
[`MediaReference`](src/otio/nodes.rs)) — the enum's tag handles the
discriminator on serialize. To avoid `serde_json` emitting the field
twice, those types' Rust structs do **not** carry an inline
`otio_schema` field.

Three node types ([`Timeline`], [`Effect`], [`Marker`]) are never
nested inside a tagged enum — they always appear as a top-level value
or inside a homogeneous `Vec`. They keep an explicit `otio_schema`
field that serializes directly.

[`Stack`] is the awkward case: it lives at `Timeline.tracks` (no enum
wrapper) AND inside [`StackChild::Stack`] / [`TrackChild::Stack`] (enum
wrapper). Solution: `Stack` itself has no inline schema field; when
referenced as `Timeline.tracks` we use the [`stack_at_root`](src/otio/nodes.rs)
serde helper module to inject and consume the `OTIO_SCHEMA: "Stack.1"`
key.

## Adding a new OTIO type: worked example

Suppose Week 5 needs `LinearTimeWarp.1` for speed-ramp effects.

### Step 1. Add the type definition in `src/otio/nodes.rs`.

```rust
/// OTIO `LinearTimeWarp.1`. Specialization of [`Effect`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinearTimeWarp {
    pub name: String,
    pub effect_name: String,
    pub time_scalar: f64,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl LinearTimeWarp {
    pub(crate) fn validate(
        &self, file: &str, path: &JsonPath
    ) -> Result<(), ProtoError> {
        if self.time_scalar <= 0.0 {
            return Err(ProtoError::validation(
                file,
                path.field("time_scalar"),
                format!("time_scalar must be > 0, got {}", self.time_scalar),
            ));
        }
        Ok(())
    }
}
```

### Step 2. Update the supported-names list in `src/otio/schema.rs`.

```rust
pub const SUPPORTED_SCHEMA_NAMES: &[OtioSchemaName] = &[
    // ...existing entries...
    OtioSchemaName { name: "LinearTimeWarp", expected_major: 1 },
];
```

### Step 3. Decide where it appears in the type tree.

`LinearTimeWarp` is an effect, so the natural slot is *inside `Effect`*.
Either:

- (a) Make `Effect` an enum that includes a `LinearTimeWarp` variant.
- (b) Keep `Effect` as the base type and add `LinearTimeWarp` as a
  parallel base type, both reachable from `Clip.effects: Vec<EffectKind>`
  where `EffectKind` is a tagged enum.

Option (b) preserves `Effect` as the v1 base type without retroactive
churn. v1 files without time warps continue to round-trip exactly. It
matches OTIO's own model where specializations are sibling types.

### Step 4. Add tests.

A roundtrip test, a validation test (negative `time_scalar` rejected),
and a forward-compat test (`LinearTimeWarp.2` warns and reads as
`LinearTimeWarp.1`).

### Step 5. Update `OTIO_NOTES.md`.

Move the row from "What we deliberately skip" to "What we model".

### What did NOT change

- The schema-versioning rules.
- Any existing type's `OTIO_SCHEMA` discriminator.
- The forward-compat warning machinery.
- The `montage` metadata namespace.

The new type is purely additive. Files written by Week 4 still round-trip
clean; files written by Week 5 with a `LinearTimeWarp` effect are
backward-incompatible only with engines older than Week 5 — which is the
correct behavior for a new schema name.

## See also

- [`src/otio/`](src/otio/) — Rust source for the typed model.
- [`src/montage_meta.rs`](src/montage_meta.rs) — `metadata.montage` types.
- [`INDEX_SCHEMA.md`](INDEX_SCHEMA.md) — sister doc for the index sidecar
  contract.

## Appendix: schema-discriminator handling in `src/otio/nodes.rs`

In OTIO JSON every object carries an `OTIO_SCHEMA` field that names its
type and major version. Three cases in our model:

1. **Standalone, never inside a tagged enum.** `Timeline`, `Effect`,
   `Marker` always serialize as themselves and own an explicit
   `otio_schema` field.
2. **Inside a `#[serde(tag = "OTIO_SCHEMA")]` enum.** `Track`, `Clip`,
   `Gap`, `ExternalReference`, `MissingReference`, `Transition` only ever
   appear inside `StackChild`, `TrackChild`, or `MediaReference`, so the
   enum's tag handles the schema string. They have no inline field.
3. **Both.** `Stack` appears in `StackChild` / `TrackChild` AND standalone
   as `Timeline::tracks`. Its struct definition has no inline schema
   field; when used standalone we go through the `stack_at_root`
   (de)serialize helper.

Schema-string forward-compat is handled at *load* time by
[`src/project.rs`](src/project.rs)'s `read_otio_timeline`, which rewrites
known-name-unknown-major schemas to the supported major before the typed
deserialize runs. This is what lets us accept e.g. `Clip.7` as
forward-compat without a custom `Deserialize` impl per type.
