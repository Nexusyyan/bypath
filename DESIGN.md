# Bypath Design System (`DESIGN.md`)

Minimalist, calm, light-first desktop instrument aesthetic aligned with Antigravity.

## Brand Identity & Logo

- **Name**: `Bypath`
- **Visual Motif**: Geometric Split / Side-Channel Path (`assets/bypath-logo.svg`, `assets/bypath-banner.svg`, `apps/desktop/src/components/Icons.tsx` `BrandMark`).
- **Geometry**: Rounded charcoal tile (`#1A1A1D`, `rx="5.4"` on `24x24` viewBox) housing an untouched vertical main trunk (`#6C6C77`, `1.75px` stroke) and a highlighted active bypass side-lane curving cleanly upward into an open chevron (`#FFFFFF`, `1.8px` stroke). Designed to remain legible from `16x16` Windows system tray icons up to `512x512` GitHub branding assets.

## Color Tokens

Strictly achromatic light neutral hierarchy with zero cool-blue tint, zero pure `#000000`, zero gradients, and zero flashy saturated labels.

| Token | Value | Role |
| :--- | :--- | :--- |
| `--surface-canvas` | `#FFFFFF` | Primary main content canvas |
| `--surface-sidebar` | `#F9F9FA` | Left navigation rail, unified titlebar, subtle table headers & pagination footers |
| `--surface-subtle` | `#F7F7F9` | Inset control / search input background |
| `--surface-hover` | `#F2F2F5` | Quiet hover wash |
| `--surface-active` | `#EAEAEF` | Active navigation pill, active process tab, and secondary action fill |
| `--surface-chip` | `#F3F3F6` | Neutral inline tags (`SNI`, `TCP`, `Process Tree`, `Domain`) |
| `--ink-primary` | `#1A1A1D` | Primary text, headings, primary CTA button fill, active switches, brand emblem |
| `--ink-medium` | `#4A4A52` | Navigation labels and secondary data values |
| `--ink-secondary` | `#6E6E77` | Metadata, descriptions, placeholders (WCAG AA >= 4.5:1 on `#FFFFFF` / `#F9F9FA`) |
| `--line-hairline` | `rgba(0, 0, 0, 0.06)` | Default structural dividers, card outlines, table rows |
| `--line-control` | `rgba(0, 0, 0, 0.10)` | Input and secondary button outlines |
| `--line-focus` | `rgba(26, 26, 29, 0.34)` | Keyboard focus outline |
| `--status-ok` | `#27794B` | 7px indicator dot only (Running / Online / Live process tab) |
| `--status-warn` | `#9A5B13` | 7px indicator dot only (Needs Admin / Warning) |
| `--status-err` | `#B43232` | 7px indicator dot and concise blocked/error text only |
| `--status-idle` | `#A1A1AA` | 7px indicator dot (Standby / Closed) |

## Geometry & Depth

- **Depth strategy**: Hairline borders only (`1px solid rgba(0, 0, 0, 0.06)`), zero default drop shadows.
- **Border radius scale**:
  - Controls (buttons, inputs, nav pills, segmented tabs, process tabs, domain chips, pagination buttons): `8px` (`--radius-sm`) and `10px` (`--radius-md`)
  - Panels, table frames, preset cards, inline composers: `14px` (`--radius-lg`) and `16px` (`--radius-xl`)
  - Status dots, process count badges, and switches: `999px` (`--radius-pill`)
- **Spacing**: `4px` base unit (`4, 8, 12, 16, 20, 24, 32`). Unified `#F9F9FA` L-frame (`44px` custom titlebar + `228px` sidebar) cradling the `#FFFFFF` canvas (`14px` top-left radius).

## Key Surface Patterns

- **Process Tabs Bar**: Horizontal scrollable rail above the Connections toolbar showing `All Processes` + per-process tabs with live indicator dot and tabular connection count.
- **Sortable Table + Integrated Pagination**: Clickable column headers with monochrome SVG sort indicators paired with a compact toolbar sort selector; integrated `#F9F9FA` pagination bar anchored inside the bottom edge of `.table-wrap`.
- **Domain Blocking Panel**: Quiet single-row quick-block bar (`*.example.com` + optional process scope) with removable monochrome domain chips and one-click domain blocking inside expanded connection rows.

## Typography & Iconography

- **Sans**: `"Segoe UI Variable Text", "Segoe UI", -apple-system, BlinkMacSystemFont, sans-serif` (`13.5px` base body, `15px / 600` section headings, `18px / 600` tabular telemetry readouts).
- **Mono**: `"Cascadia Mono", "Consolas", ui-monospace, monospace` with `font-variant-numeric: tabular-nums`, reserved strictly for PIDs, SNI hostnames, domain patterns, IP:port endpoints, file paths, and byte counters.
- **Icons**: Custom monochrome `1.65px` stroke SVGs in `apps/desktop/src/components/Icons.tsx` (zero emojis or Unicode glyphs).
