# UI/UX Part 22 — Design System, Tokens, Typography, Icons & Motion Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 22  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define the complete cross-platform design-system architecture covering semantic tokens, typography, spacing, layout grids, shape, elevation, iconography, color roles, dark/light/high-contrast behavior, motion, reduced-motion rules, responsive density, component primitives, plugin theming boundaries, accessibility constraints, token versioning, and the shared contracts that preserve one product language without forcing one rendered UI across desktop and Android.

---

# 1. Purpose

A reusable communication platform needs a consistent visual and interaction language across:

```text
messaging
calls
files
search
contacts
groups
security
backup
emergency
settings
plugins
diagnostics
```

while still allowing:

```text
desktop-native interaction
Android-native interaction
platform-specific layout
platform-specific typography rendering
platform-specific controls
```

The governing principle is:

> **Share semantic design intent, not pixel-perfect UI implementation.**

The desktop and Android apps should feel like the same product without pretending they are the same operating system.

---

# 2. Architectural Position

```text
Shared Semantic Design Language
          │
          ├── color roles
          ├── typography roles
          ├── spacing roles
          ├── shape roles
          ├── elevation roles
          ├── motion roles
          ├── icon semantics
          └── component behavior
          │
      ┌───┴──────────┐
      │              │
Dioxus Theme      Compose Theme
Desktop           Android
      │              │
Platform-Native Rendering
```

---

# 3. What Is Shared

Share:

```text
semantic names
interaction meaning
accessibility constraints
component states
visual hierarchy
motion intent
status semantics
icon meaning
```

---

# 4. What Is Not Shared

Do not force identical:

```text
font metrics
pixel sizes
window chrome
system bars
touch target treatment
dialog layout
navigation pattern
hover behavior
elevation implementation
```

---

# 5. Design Token Families

Recommended:

```text
Color
Typography
Spacing
Sizing
Shape
Elevation
Motion
Opacity
Border
Focus
Icon
Z-order
```

---

# 6. Semantic Token Principle

Bad:

```text
blue500
gray700
spacing12
```

for component logic.

Good:

```text
surface_primary
text_secondary
action_primary
status_error
space_compact
```

---

# 7. Primitive vs Semantic Tokens

Use two levels:

```text
primitive tokens
semantic tokens
```

---

# 8. Primitive Tokens

Examples:

```text
color neutral 0–1000
spacing numeric scale
radius numeric scale
duration numeric scale
```

---

# 9. Semantic Tokens

Examples:

```text
surface.base
surface.raised
surface.overlay
text.primary
text.secondary
text.disabled
action.primary
action.destructive
status.success
status.warning
status.error
status.security
status.emergency
```

---

# 10. Components Consume Semantic Tokens

Hard rule.

Component code should not directly depend on raw palette indexes unless inside theme implementation.

---

# 11. Color Roles

Recommended baseline:

```rust
pub enum ColorRole {
    SurfaceBase,
    SurfaceRaised,
    SurfaceOverlay,
    SurfaceSelected,
    SurfaceHover,
    SurfacePressed,
    TextPrimary,
    TextSecondary,
    TextMuted,
    TextInverse,
    BorderSubtle,
    BorderStrong,
    ActionPrimary,
    ActionPrimaryText,
    ActionSecondary,
    ActionDestructive,
    StatusSuccess,
    StatusWarning,
    StatusError,
    StatusSecurity,
    StatusEmergency,
    FocusRing,
}
```

---

# 12. Status Colors Must Be Semantic

Use separate roles for:

```text
error
warning
security
emergency
success
```

Do not assume all important states are red.

---

# 13. Color Independence

Every status state must also use:

```text
text
icon
shape/border
```

where needed.

---

# 14. Light Theme

Should preserve:

```text
clear hierarchy
comfortable contrast
subtle surfaces
```

---

# 15. Dark Theme

Must not simply invert colors.

Adjust:

```text
surface hierarchy
contrast
elevation cues
border strength
media backgrounds
```

---

# 16. System Theme

Default recommendation:

```text
Follow System
```

---

# 17. Theme Modes

```rust
pub enum ThemeMode {
    System,
    Light,
    Dark,
}
```

---

# 18. High Contrast

Separate accessibility profile.

Not equivalent to:

```text
dark theme
```

---

# 19. High-Contrast Theme

Increase:

```text
text contrast
focus visibility
border clarity
state distinction
```

---

# 20. Accent Color

Optional user setting.

Must not override:

```text
error
warning
security
emergency
```

semantic colors.

---

# 21. Android Dynamic Color

Can map Material dynamic color into semantic roles.

---

# 22. Dynamic Color Boundaries

Dynamic color must still satisfy:

```text
contrast
security-state distinction
emergency-state distinction
```

---

# 23. Desktop Accent

May follow system or user-selected theme.

---

# 24. Brand Identity

Brand color should remain limited to:

```text
primary action
selected state
brand surfaces
```

not overwhelm content.

---

# 25. Typography Architecture

Use semantic roles.

---

# 26. Typography Roles

Recommended:

```rust
pub enum TypographyRole {
    Display,
    TitleLarge,
    TitleMedium,
    TitleSmall,
    BodyLarge,
    BodyMedium,
    BodySmall,
    LabelLarge,
    LabelMedium,
    LabelSmall,
    Monospace,
}
```

---

# 27. Platform Mapping

Desktop and Android can map same semantic role to different exact metrics.

---

# 28. Android Typography

Base on Material 3 typography where practical.

---

# 29. Desktop Typography

Optimize for:

```text
dense conversation lists
long-form message reading
settings
diagnostics
```

---

# 30. Font Choice

Prefer platform-available/system fonts unless strong brand need.

Benefits:

```text
performance
native rendering
language coverage
accessibility
```

---

# 31. No Bundled Font Requirement

Do not make product depend on one proprietary font.

---

# 32. Monospace Role

Use for:

```text
IDs
codes
fingerprints
diagnostics
logs
```

---

# 33. Do Not Use Monospace for Normal Messages

Unless user chooses it.

---

# 34. Typography Scale

Maintain clear hierarchy.

Avoid excessive number of sizes.

---

# 35. Text Scaling

Must support accessibility scaling.

---

# 36. Android Font Scale

Respect OS font size.

---

# 37. Desktop UI Scale

Respect OS scale and optional app-level scaling.

---

# 38. Large Text Behavior

Components expand vertically.

Critical text never truncates.

---

# 39. Line Height

Optimize for readability.

Recommended relative range:

```text
1.3–1.6
```

depending role.

---

# 40. Message Body Typography

Needs comfortable:

```text
line height
paragraph spacing
selection
mixed-script rendering
```

---

# 41. Technical Text

May use smaller line height but still accessible.

---

# 42. Weight

Use restrained weights.

Recommended:

```text
regular
medium
semibold
```

rather than many weights.

---

# 43. Bold Accessibility

Do not use bold alone to communicate state.

---

# 44. Italic

Avoid for critical/status text.

---

# 45. Spacing System

Use a consistent scale.

Example semantic scale:

```rust
pub enum SpaceToken {
    None,
    Xs,
    Sm,
    Md,
    Lg,
    Xl,
    Xxl,
}
```

---

# 46. Primitive Spacing Example

Possible:

```text
2
4
8
12
16
24
32
48
```

platform adjusted.

---

# 47. Component Padding

Use semantic spacing.

---

# 48. Message Bubble Spacing

Separate:

```text
within-message spacing
between grouped messages
between sender groups
```

---

# 49. Desktop Density

Support:

```rust
pub enum DensityMode {
    Comfortable,
    Compact,
}
```

---

# 50. Android Density

Follow touch-first defaults.

Do not expose compact mode unless carefully tested.

---

# 51. Minimum Targets

Android:

```text
48 dp
```

recommended minimum interactive target.

Desktop:

```text
comfortable mouse + keyboard target
```

without wasting space.

---

# 52. Layout Grid

Use simple responsive units.

Recommended desktop:

```text
4/8 px logical grid
```

Android:

```text
Material spacing alignment
```

---

# 53. Responsive Width Tokens

Possible semantic breakpoints:

```rust
pub enum WindowClass {
    Compact,
    Medium,
    Wide,
    UltraWide,
}
```

---

# 54. Desktop Shell Widths

Define semantic ranges for:

```text
rail
sidebar
main workspace
inspector
```

not hardcoded per screen.

---

# 55. Android Adaptive Width

Use:

```text
compact
medium
expanded
```

mapped to Compose window size classes.

---

# 56. Shape System

Recommended roles:

```rust
pub enum ShapeRole {
    None,
    Small,
    Medium,
    Large,
    Pill,
    Circle,
}
```

---

# 57. Shape Consistency

Use:

```text
small radius for compact controls
medium for cards/bubbles
large for sheets/dialog surfaces
pill for chips/status
```

---

# 58. Do Not Over-Round Everything

Too much rounding reduces hierarchy.

---

# 59. Message Bubble Shape

Can be product-specific but should preserve:

```text
grouping
sender distinction
reply nesting
```

---

# 60. Elevation System

Use sparingly.

---

# 61. Elevation Roles

```rust
pub enum ElevationRole {
    Flat,
    Raised,
    Floating,
    Modal,
}
```

---

# 62. Desktop Elevation

May use:

```text
border
shadow
surface contrast
```

---

# 63. Android Elevation

Map naturally to Material elevation.

---

# 64. Dark Theme Elevation

Often needs:

```text
surface tonal difference
```

rather than heavy shadows.

---

# 65. Border System

Recommended:

```rust
pub enum BorderRole {
    None,
    Subtle,
    Strong,
    Focus,
    Warning,
    Error,
}
```

---

# 66. Divider Usage

Avoid excessive dividers.

Prefer spacing and surface hierarchy.

---

# 67. Focus Tokens

Critical for desktop.

---

# 68. Focus Ring

Must have:

```text
high contrast
consistent thickness
clear offset
```

---

# 69. Focus Ring Token

```rust
pub struct FocusToken {
    pub width: f32,
    pub offset: f32,
    pub role: ColorRole,
}
```

---

# 70. Focus vs Selected

Do not use identical visual treatment.

---

# 71. Hover State

Desktop only.

---

# 72. Hover Tokens

Subtle.

Hover must not be required for discovering essential actions.

---

# 73. Pressed State

Both platforms.

---

# 74. Disabled State

Needs:

```text
reduced emphasis
still readable
```

---

# 75. Selected State

Strong enough for:

```text
conversation
settings category
tab
navigation destination
```

---

# 76. Error State

Input fields should combine:

```text
border
icon
text
```

---

# 77. Warning State

Likewise.

---

# 78. Security State

Distinct from generic error.

Example semantic role:

```text
identity changed
```

---

# 79. Emergency State

Reserved strongest attention treatment.

---

# 80. Iconography Architecture

Icons represent semantic action/state.

---

# 81. Icon Role Examples

```text
send
attach
call
video
search
settings
security
backup
device
file
warning
error
emergency
```

---

# 82. Icon Set Strategy

Prefer one coherent base icon family per platform.

Android may use Material Symbols/icons.

Desktop can use matching/open icon set.

---

# 83. Cross-Platform Icon Semantics

Icon drawing can differ.

Meaning must match.

---

# 84. Icon Naming

Use semantic names:

```text
icon.send
icon.device.link
icon.security.warning
```

---

# 85. Icon-Only Controls

Always accessible label.

---

# 86. Decorative Icons

Do not enter accessibility tree.

---

# 87. Status Icons

Need distinct silhouettes.

Do not rely on color.

---

# 88. Verification Icons

Separate:

```text
verified
unverified
identity changed
```

---

# 89. Message State Icons

Separate:

```text
queued
sent
delivered
read
failed
```

---

# 90. File Type Icons

Useful fallback.

---

# 91. Plugin Icons

Sandboxed/bounded assets.

---

# 92. User-Supplied Icons

Never allowed to spoof core security/emergency icons.

---

# 93. Reserved Icon Namespace

Core owns:

```text
security
verification
emergency
device trust
```

---

# 94. Icon Size Tokens

Recommended:

```rust
pub enum IconSize {
    Small,
    Medium,
    Large,
    Hero,
}
```

---

# 95. Inline Icons

Align with text baseline.

---

# 96. Motion Architecture

Motion should explain:

```text
continuity
state change
navigation
cause/effect
```

not decorate everything.

---

# 97. Motion Roles

```rust
pub enum MotionRole {
    Instant,
    Fast,
    Standard,
    Slow,
    Emphasis,
}
```

---

# 98. Example Durations

Possible:

```text
Instant: 0
Fast: 80–120 ms
Standard: 160–240 ms
Slow: 280–400 ms
```

Tune per platform.

---

# 99. Reduced Motion

Map all motion roles to:

```text
Instant
or
minimal fade
```

as appropriate.

---

# 100. Navigation Motion

Desktop:

```text
subtle/mostly instant
```

Android:

```text
platform-native transitions
```

---

# 101. Message Arrival Motion

Subtle.

Must not disturb scroll/focus.

---

# 102. Presence Motion

No repeated pulsing.

---

# 103. Typing Motion

Optional animated dots.

Reduced mode:

```text
static text
```

---

# 104. Transfer Progress

Smooth progress allowed.

No excessive animation.

---

# 105. Call Reconnect

Subtle status animation.

---

# 106. SOS

No flashing/pulsing requirement.

---

# 107. Security Warning

No shaking/strobing.

---

# 108. Motion Token API

```rust
pub struct MotionToken {
    pub duration_ms: u32,
    pub easing: MotionEasing,
}
```

---

# 109. Easing

Use few consistent curves:

```rust
pub enum MotionEasing {
    Linear,
    Standard,
    Decelerate,
    Accelerate,
}
```

---

# 110. Spring Motion

Use cautiously.

Do not depend on it for meaning.

---

# 111. Animation Performance

Must not block:

```text
input
message rendering
call controls
```

---

# 112. GPU-Friendly Motion

Prefer:

```text
opacity
transform
```

over expensive layout animations.

---

# 113. Component Design System

Shared component semantics:

```text
Button
IconButton
TextField
SearchField
ListRow
MessageBubble
Chip
Badge
Banner
Card
Dialog
Sheet
Menu
Tooltip
Tabs
Progress
Avatar
StatusIndicator
```

---

# 114. Component Contract

Each shared semantic component defines:

```text
purpose
states
accessibility
spacing
typography
platform adaptations
```

---

# 115. Button Variants

Recommended:

```rust
pub enum ButtonVariant {
    Primary,
    Secondary,
    Tertiary,
    Destructive,
}
```

---

# 116. Button States

```text
default
hover desktop
pressed
focused
disabled
loading
```

---

# 117. Loading Button

Should keep stable width where possible.

Accessible state:

```text
busy
```

---

# 118. Destructive Button

Use restrained destructive styling.

Not every danger action needs full red fill.

---

# 119. Text Field States

```text
default
focused
filled
error
disabled
read-only
```

---

# 120. Search Field

Different semantic purpose from generic text field.

---

# 121. List Row

Supports:

```text
leading
title
subtitle
metadata
badge
trailing action
selected
```

---

# 122. Conversation Row

Extends ListRow but has specialized:

```text
unread
typing
draft
send failure
presence
```

---

# 123. Message Bubble

Specialized component.

Must support:

```text
text
reply
attachments
reactions
status
selection
```

---

# 124. Avatar

Supports:

```text
image
initial/fallback
group stack
status overlay
verification overlay
```

---

# 125. Avoid Too Many Avatar Overlays

Priority:

```text
verification warning > presence
```

or use adjacent status instead.

---

# 126. Badge

Reserved for:

```text
count
short status
```

not long text.

---

# 127. Chip

Used for:

```text
filter
status
category
```

---

# 128. Banner

Used for:

```text
security warning
offline state
backup warning
emergency
```

---

# 129. Banner Severity

```rust
pub enum BannerSeverity {
    Info,
    Success,
    Warning,
    Error,
    Security,
    Emergency,
}
```

---

# 130. Dialog

Use for focused decisions.

---

# 131. Full-Screen Flow

Use instead of dialog for:

```text
device linking
recovery
restore
complex group creation
```

on mobile.

---

# 132. Sheet

Android:

```text
bottom sheet
```

for contextual actions.

Desktop:

```text
popover/menu/dialog
```

not forced bottom sheet.

---

# 133. Tooltip

Desktop helpful.

Never required for essential meaning.

---

# 134. Toast/Snackbar

Short-lived feedback.

Persistent issues need banner/inline state.

---

# 135. Progress Components

```text
linear
circular
indeterminate
step
```

Use semantic phase where possible.

---

# 136. Skeleton Loading

Only for known layout.

Do not use everywhere.

---

# 137. Empty State Component

Contains:

```text
title
description
optional action
```

---

# 138. Error State Component

Contains:

```text
what failed
retry/recovery
```

---

# 139. Offline State Component

Contains:

```text
what still works
what is waiting
```

---

# 140. Design Token Storage

Shared semantic definitions should live in a versioned schema.

Potential crate:

```text
comm-design-tokens
```

---

# 141. Token Schema

Example:

```rust
pub struct DesignTokenSet {
    pub colors: SemanticColorTokens,
    pub typography: TypographyTokens,
    pub spacing: SpacingTokens,
    pub shapes: ShapeTokens,
    pub elevation: ElevationTokens,
    pub motion: MotionTokens,
    pub focus: FocusTokens,
}
```

---

# 142. Rust Ownership

Rust can define semantic token schema and stable identifiers.

Platform UI can map them to actual native values.

---

# 143. Avoid Runtime Overcoupling

Do not require Rust to compute every pixel/style at runtime.

---

# 144. Generated Token Artifacts

Possible pipeline:

```text
shared token source
→ Rust constants/schema
→ Compose theme definitions
→ Dioxus theme definitions
```

---

# 145. Source Format

Use human-readable:

```text
RON
```

if desired.

---

# 146. Example Token Source

```ron
(
    color_roles: (
        text_primary: ...,
        surface_base: ...,
    ),
    spacing: (
        sm: 8,
        md: 16,
    ),
)
```

---

# 147. Build-Time Generation

Recommended.

---

# 148. Runtime Theme Switching

Semantic theme values can switch at runtime.

---

# 149. Token Versioning

Every token set has:

```text
schema version
theme version
```

---

# 150. Token Compatibility

Plugins consume semantic tokens, not raw internal palette.

---

# 151. Plugin Theme API

Plugins can request:

```text
surface
text
action
status
spacing
typography
```

semantic roles.

---

# 152. Plugin Restrictions

Plugins cannot override:

```text
core security colors
emergency iconography
global font
focus visibility
```

---

# 153. Plugin UI Consistency

Declarative plugin surfaces inherit host theme automatically.

---

# 154. Theme Change

Plugins update with host.

---

# 155. Theme Isolation

Plugin cannot force light theme inside dark app unless special media content.

---

# 156. Brand Customization

Future managed deployments may allow:

```text
logo
accent
organization name
```

---

# 157. Managed Branding

Must not compromise:

```text
contrast
security semantics
emergency semantics
```

---

# 158. White-Label Boundaries

Do not allow replacement of:

```text
verification icons
security terminology
SOS semantics
```

if it would mislead users.

---

# 159. Responsive Design Tokens

Some semantic sizes vary by:

```text
window class
density mode
platform
```

---

# 160. Responsive Token Resolver

```rust
pub struct ResponsiveContext {
    pub platform: PlatformKind,
    pub window_class: WindowClass,
    pub density: DensityMode,
    pub text_scale: f32,
}
```

---

# 161. Token Resolution

Platform UI resolves:

```text
semantic token
+
responsive context
→ concrete value
```

---

# 162. Message Width

Recommended:

```text
max readable bubble width
```

not full-width on wide desktop.

---

# 163. Long-Form Text

Use readable line-length limits.

---

# 164. Settings Width

Avoid overly wide lines.

---

# 165. Diagnostics Tables

Can use wider width.

---

# 166. Desktop Multi-Pane

Tokens define:

```text
rail width
sidebar min/max
inspector width
content max
```

---

# 167. Android Compact

Bottom navigation.

---

# 168. Android Expanded

Navigation rail + list/detail.

---

# 169. Foldable Hinge

Layout tokens can include safe pane gap.

---

# 170. Safe Areas

Android:

```text
system bars
display cutout
IME
```

Desktop:

```text
window chrome
```

---

# 171. IME Insets

Composer respects keyboard.

---

# 172. Visual Density Hierarchy

Frequent information should be compact.

High-risk decisions should have more whitespace.

---

# 173. Security Screen Density

Comfortable.

---

# 174. Diagnostics Density

Can be compact in developer mode.

---

# 175. Emergency Density

Minimal, high clarity.

---

# 176. Image Treatment

Use:

```text
rounded preview
neutral placeholder
aspect-ratio reservation
```

---

# 177. Sensitive Media

Do not auto-preview if privacy policy says hidden.

---

# 178. Placeholder Tokens

Use semantic:

```text
media_placeholder
avatar_placeholder
```

---

# 179. Loading Images

Avoid layout shift.

---

# 180. Avatar Palette

Fallback avatar colors must preserve text contrast.

---

# 181. Random Avatar Color

Can derive deterministically from stable non-secret identifier.

---

# 182. Do Not Encode Trust in Avatar Color

---

# 183. Shadows

Minimal.

Avoid huge soft shadows that reduce clarity/performance.

---

# 184. Blur

Use cautiously.

Can hurt performance and accessibility.

---

# 185. Transparency

Ensure contrast with varied content.

---

# 186. Glassmorphism

Not recommended for core communication surfaces.

---

# 187. Visual Hierarchy

Priority:

```text
content
action
status
decoration
```

---

# 188. Content First

Messages/files/calls should dominate, not chrome.

---

# 189. Calm Interface

Avoid excessive badges, colors, borders, animation.

---

# 190. Security Attention

Reserve stronger visual treatment so it matters.

---

# 191. Emergency Attention

Strongest reserved treatment.

---

# 192. Design-System Accessibility Rules

Every token/component must satisfy Part 21.

---

# 193. Component Accessibility Checklist

Required:

```text
label
role
focus
keyboard/touch
contrast
large text
RTL
reduced motion
screen reader
```

---

# 194. Component State Matrix

Every component spec documents:

```text
default
hover
focus
pressed
selected
disabled
loading
error
```

where relevant.

---

# 195. Visual Regression Tests

Screenshot/golden tests for:

```text
light
dark
high contrast
large text
RTL
compact desktop
wide desktop
Android compact
Android expanded
```

---

# 196. Token Unit Tests

Validate:

```text
missing semantic role
duplicate token
invalid contrast pairing
unsupported state mapping
```

---

# 197. Contrast Tests

Automate where possible.

---

# 198. Typography Tests

Verify:

```text
line wrapping
CJK
Arabic
Urdu
emoji
long names
technical IDs
```

---

# 199. Icon Tests

Verify:

```text
semantic mapping
accessible labels
dark/light visibility
```

---

# 200. Motion Tests

Verify reduced-motion mode disables/reduces animation.

---

# 201. Responsive Tests

Check:

```text
320-ish phone width
large phones
tablets
foldables
desktop small window
desktop ultrawide
```

---

# 202. Density Tests

Desktop compact must not break accessibility minimums.

---

# 203. Plugin Theme Tests

Plugin extension remains readable in:

```text
light
dark
high contrast
large text
RTL
```

---

# 204. Token Governance

Changes to shared semantic tokens require review.

---

# 205. Breaking Token Change

Examples:

```text
rename semantic role
remove status role
change component contract
```

requires version bump/migration.

---

# 206. Non-Breaking Theme Change

Palette adjustment under same semantic roles.

---

# 207. Design Review

Every new screen should reference:

```text
existing tokens
existing components
existing patterns
```

before creating new ones.

---

# 208. Component Proliferation Rule

Do not create:

```text
10 button styles
7 card types
5 banner systems
```

without semantic need.

---

# 209. Component Naming

Use function:

```text
SecurityBanner
```

only if behavior differs materially.

Otherwise:

```text
Banner(severity = Security)
```

---

# 210. Cross-Platform Component Mapping

Example:

```text
Semantic: ContextActions

Desktop:
    context menu / toolbar / inspector

Android:
    bottom sheet / overflow menu
```

---

# 211. Cross-Platform Navigation Mapping

Semantic destination shared.

Visual navigation differs.

---

# 212. Cross-Platform Dialog Mapping

Semantic confirmation shared.

Android may use dialog/full-screen sheet.

Desktop may use modal dialog.

---

# 213. Cross-Platform Status Mapping

Semantic state shared.

Visual representation adapts.

---

# 214. Design System Presentation API

The design system itself should not require a heavy runtime API.

A lightweight shared contract is sufficient.

---

# 215. Theme Identity

```rust
pub struct ThemeDescriptor {
    pub mode: ThemeMode,
    pub high_contrast: bool,
    pub accent: Option<AccentChoice>,
    pub density: DensityMode,
}
```

---

# 216. Theme Settings API

```rust
pub trait ThemePresentation {
    async fn theme(
        &self,
    ) -> Result<ThemeDescriptor, UiError>;

    async fn update(
        &self,
        update: ThemeUpdate,
    ) -> Result<ThemeDescriptor, UiError>;
}
```

---

# 217. Theme Event

```rust
pub enum ThemeUiEvent {
    ThemeChanged(ThemeDescriptor),
}
```

---

# 218. Platform Theme Adapter

Desktop/Android resolve actual palette/metrics.

---

# 219. Accessibility Integration

Theme resolver consumes:

```text
high contrast
text scale
reduced motion
```

---

# 220. Design Token Build Pipeline

Recommended:

```text
tokens.ron
   │
   ├── validate schema
   ├── contrast checks
   ├── generate Rust token IDs
   ├── generate Compose theme values
   └── generate Dioxus theme values
```

---

# 221. Token Validation Tool

Should fail build on:

```text
missing required semantic token
invalid numeric range
contrast regression
duplicate key
unknown schema version
```

---

# 222. Theme Preview Tool

Developer utility can preview:

```text
all components
all states
all themes
all densities
```

---

# 223. Component Gallery

Desktop and Android internal developer screen.

---

# 224. Component Gallery Contents

```text
buttons
inputs
lists
banners
dialogs
chips
badges
avatars
message bubbles
call controls
progress
empty/error states
```

---

# 225. Design QA

Use component gallery for screenshot regression.

---

# 226. Figma Dependency

Architecture should not require a design tool at runtime.

Design specs can be mirrored in Figma if team uses it.

---

# 227. Source of Truth

Code token schema should be authoritative for implemented tokens.

---

# 228. Documentation

Each token should document:

```text
meaning
where used
where not used
```

---

# 229. Example

```text
status.emergency:
Use only for active emergency/SOS context.
Do not use for ordinary destructive actions.
```

---

# 230. Typography Documentation

Each role documents:

```text
purpose
platform mapping
minimum scale behavior
```

---

# 231. Motion Documentation

Each role documents:

```text
purpose
duration intent
reduced-motion fallback
```

---

# 232. Icon Documentation

Each icon semantic documents:

```text
meaning
reserved usage
accessible label expectation
```

---

# 233. Design-System Telemetry

None required.

Do not collect theme/accessibility choices as profiling by default.

---

# 234. Safe Diagnostics

Can detect:

```text
token resolution failure
missing asset
theme load error
```

without user profiling.

---

# 235. Runtime Fallback

If custom theme fails:

```text
fall back to safe default semantic theme
```

---

# 236. Missing Icon

Use generic safe fallback.

Do not crash.

---

# 237. Unsupported Font

Fallback to system font.

---

# 238. Plugin Asset Failure

Plugin extension falls back gracefully.

---

# 239. Theme Migration

Old theme setting migrates safely.

---

# 240. Accent Migration

If chosen accent removed:

```text
fall back to system/default
```

---

# 241. High Contrast Priority

High contrast overrides aesthetic accent choices when necessary.

---

# 242. Reduced Motion Priority

Accessibility setting overrides animation preference.

---

# 243. System Theme Change

Live update.

---

# 244. Android Dynamic Color Change

Live update if platform supports.

---

# 245. Desktop Appearance Change

Live update where possible.

---

# 246. Message Theme Stability

Theme switch must not alter message ordering/layout anchor.

---

# 247. Call Theme Stability

Theme switch must not disrupt active call/media pipeline.

---

# 248. Emergency Theme Stability

Emergency state remains unmistakable in all themes.

---

# 249. Security Theme Stability

Identity-change warning remains unmistakable.

---

# 250. Initial Production Scope

Ship:

```text
semantic color tokens
light/dark/system
high-contrast support
typography roles
spacing scale
shape roles
elevation roles
focus system
icon semantics
motion/reduced-motion roles
desktop comfortable/compact density
Android adaptive Material 3 mapping
shared component contracts
theme/token build validation
plugin theme inheritance
component gallery
visual regression tests
```

Defer:

```text
full theme marketplace
arbitrary user CSS
complex visual skinning
animated theme packs
deep white-label overrides
```

---

# 251. Definition of Done

UI/UX Part 22 is complete when:

- desktop and Android share semantic design intent rather than pixel-perfect implementation
- primitive and semantic token layers are distinct
- components consume semantic tokens
- color roles cover surfaces, text, actions, status, focus, security, and emergency
- light, dark, system, and high-contrast behavior are defined
- Material dynamic color cannot weaken security/emergency semantics
- typography roles, large-text behavior, multilingual rendering, and monospace use are explicit
- spacing, density, shape, elevation, border, focus, and responsive tokens are defined
- icon semantics are shared while artwork may differ per platform
- reserved security/emergency icon namespaces prevent spoofing
- motion is purposeful, bounded, performant, and has reduced-motion fallbacks
- shared components define behavior/states/accessibility instead of raw visuals
- plugin surfaces inherit host semantic tokens and cannot override core security semantics
- token schemas are versioned and validated at build time
- Dioxus and Compose theme outputs can be generated from a shared source
- component gallery and visual regression coverage include light/dark/high-contrast/large-text/RTL/responsive states
- accessibility requirements from Part 21 are design-system gates
- theme switching never disrupts message anchors, calls, or emergency state
- the design system remains calm, restrained, and content-first

---

# 252. Final Architecture

```text
                    SHARED SEMANTIC DESIGN
                              │
      ┌───────────────────────┼───────────────────────┐
      │                       │                       │
    Tokens                Components              Semantics
      │                       │                       │
 Color/Type/Space       States/Behavior        Icons/Motion
 Shape/Focus            Accessibility          Hierarchy
      │                       │                       │
      └───────────────────────┼───────────────────────┘
                              │
                 Platform Theme Mapping
                    ┌─────────┴─────────┐
                    │                   │
                 Dioxus              Compose
                 Desktop             Android
                    │                   │
             Desktop-Native       Android-Native
              Rendering             Rendering
```

---

# 253. Final Principle

The design system should create a recognizable product language without fighting the platform.

The correct model is:

```text
shared semantic tokens
+
shared component meaning
+
shared accessibility rules
+
platform-native rendering
```

not:

```text
one cross-platform pixel specification copied everywhere
```

This gives the Dioxus desktop and Android Compose applications visual consistency, accessibility, maintainability, and room to remain genuinely native on each platform.
