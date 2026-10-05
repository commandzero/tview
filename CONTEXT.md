# Tview

Tview presents tabular data from file and query-native sources. This glossary distinguishes source results, local views, saved configuration, and frozen output.

## Language

**Source**:
The input from which tabular data is obtained, such as a file, standard input, a database, or a remote endpoint.

**Relation**:
A selectable table, view, index, or data stream within a source.

**Source configuration**:
The source-opening choices and source-native operations that describe a requested result. It includes relation or native query selection independently of local view settings.

**Committed source configuration**:
The complete source-opening choices and source-native operations associated with the last successfully activated source result, sufficient to describe reopening that result. A draft or pending request is not committed source configuration.
_Avoid_: Source recipe, committed recipe, latest requested configuration, draft configuration

**Source result**:
The active tabular result produced by source configuration, including its schema, typed rows, extent, and query provenance when available. “Active result” and “activated result” describe this concept's lifecycle state, not additional domain entities.

**Source generation**:
The identity scope of an opened result's rows and columns. Identities from different generations are not interchangeable.

**Source query**:
Source-native filters, ordering, and limits, optionally composed with an opaque native base query.

**Native base query**:
The configured query text before Tview composes source operations and execution bookkeeping. It is distinct from the composed query shown as provenance.

**View transform**:
Source-independent filtering and sorting of the active source result. It does not expand that result or replace source configuration.

**Saved view**:
A named configuration document containing source configuration and local view settings, selected by name or input filename.

**Selected saved-view snapshot**:
The validated saved view chosen for one invocation, shared by source configuration and presentation binding.

**Provisional schema**:
A schema whose column discovery is incomplete. Later discovery can reveal additional columns without changing existing column identities within the source generation.

**Frozen preview projection**:
The selected output rows and their presentation facts after preview preparation, together with evidence about omitted rows. Lookahead rows are not part of the emitted preview.

**Conditional color**:
A cell foreground selected by ordered column rules, using cell values and any applicable column profile. Selection and search styling are separate presentation concerns.
