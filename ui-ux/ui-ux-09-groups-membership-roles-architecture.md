# UI/UX Part 09 — Groups, Membership & Roles UX Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 09  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define the complete group UX across desktop and Android, including group creation, member selection, invites, join/leave flows, group profiles, membership requests, roles, permissions, ownership, moderation, bans, member removal, notification settings, shared media, active group calls, security cues, membership events, large-group scaling, accessibility, and the Rust presentation contracts that drive both UIs.

---

# 1. Purpose

Groups introduce a level of complexity that does not exist in direct conversations.

Users must understand:

```text
who is in the group
who can invite
who can remove members
who can rename the group
who can send messages
who owns the group
what happens when roles change
whether membership is verified
whether a join request is pending
whether someone was removed or banned
```

The governing principle is:

> **Group UX should make membership, authority, and security understandable without exposing protocol complexity.**

---

# 2. Architectural Position

```text
Rust Group Core
    │
    ├── Group identity
    ├── Membership
    ├── Roles
    ├── Permissions
    ├── Invitations
    ├── Join requests
    ├── Moderation state
    ├── Group security epochs
    └── Group calls
    │
    ▼
Group Presentation Service
    │
 ┌──┴──────────────┐
 │                 │
Dioxus           Compose
Desktop          Android
```

---

# 3. Group Types

Baseline:

```rust
pub enum GroupKind {
    Private,
    InviteOnly,
    Managed,
}
```

Future optional:

```text
Public
Channel
Broadcast
Organization
```

Do not overbuild v1.

---

# 4. Group Identity

Every group has a stable:

```text
GroupId
```

Display name/avatar can change.

The UI must never use group name as identity.

---

# 5. Group Summary

```rust
pub struct GroupSummaryView {
    pub id: GroupId,
    pub name: String,
    pub avatar: AvatarRef,
    pub member_count: u32,
    pub my_role: GroupRole,
    pub unread_count: u32,
    pub muted: bool,
    pub security: GroupSecuritySummary,
}
```

---

# 6. Group Roles

Recommended baseline:

```rust
pub enum GroupRole {
    Owner,
    Admin,
    Moderator,
    Member,
}
```

---

# 7. Role Semantics

Owner:

```text
ultimate group authority
can transfer ownership
can delete/disband group if supported
```

Admin:

```text
manage members
change group settings
manage roles
```

Moderator:

```text
moderate messages/members within limits
```

Member:

```text
normal participation
```

Exact capabilities come from Rust.

---

# 8. Do Not Infer Permissions from Role Name in UI

Rust supplies explicit capabilities.

Example:

```rust
pub struct GroupCapabilities {
    pub can_send: bool,
    pub can_invite: bool,
    pub can_remove_member: bool,
    pub can_ban_member: bool,
    pub can_change_roles: bool,
    pub can_edit_group: bool,
    pub can_manage_join_requests: bool,
    pub can_transfer_ownership: bool,
    pub can_leave: bool,
}
```

---

# 9. Group Profile

Recommended sections:

```text
Header
Members
Roles & Permissions
Shared Media
Calls
Notifications
Security
Invite / QR
Leave Group
Delete Group / Transfer Ownership
```

---

# 10. Group Header

Show:

```text
group avatar
group name
member count
description
my role
```

Actions:

```text
Message
Audio/Video Call if supported
Search
Invite
More
```

---

# 11. Group Avatar

Group admin/owner may edit.

Fallback:

```text
initials / generated composition
```

---

# 12. Group Description

Optional.

Treat as untrusted display content.

---

# 13. Group Creation Flow

Recommended:

```text
New Group
→ Select Members
→ Name Group
→ Set Avatar optional
→ Review
→ Create
```

---

# 14. Desktop Group Creation

Use:

```text
multi-pane/modal wizard
```

Member selection can use keyboard and multi-select.

---

# 15. Android Group Creation

Use:

```text
full-screen flow
```

or step-based navigation.

---

# 16. Member Selection

Search contacts locally.

Show:

```text
name
verification state if relevant
avatar
```

---

# 17. Selection Count

Show:

```text
3 selected
```

---

# 18. Max Group Size

Rust enforces.

UI can show:

```text
45 / 100 selected
```

if limit relevant.

---

# 19. Unknown/Unverified Member

Do not prevent selection unless policy requires.

But show trust state.

---

# 20. Create Command

```rust
pub struct CreateGroupCommand {
    pub name: String,
    pub members: Vec<AccountId>,
    pub avatar: Option<GroupAvatarInput>,
}
```

---

# 21. Creation Commit

Only after Rust confirms:

```text
group created
```

navigate into group.

---

# 22. Creation Failure

Preserve selected members/name.

---

# 23. Offline Group Creation

Possible if backend supports local provisional group + later sync.

If not:

```text
show requires connection
```

Do not fake success.

---

# 24. Invite Members

Entry points:

```text
Group Profile
→ Add Members
```

or:

```text
Invite Link / QR
```

---

# 25. Add Existing Contacts

Admin selects contacts.

Rust validates capability and membership state.

---

# 26. Invite Link

Can be:

```text
single-use
multi-use
expiring
role-scoped
```

depending group policy.

---

# 27. Invite QR

Encodes authenticated invite payload.

---

# 28. Invite Preview

Show before sharing:

```text
Group name
Invite expiry
Uses remaining if applicable
Who can use
```

---

# 29. Invite Revocation

Admin/owner can revoke.

---

# 30. Join via Invite

Flow:

```text
Open/Scan Invite
→ Validate
→ Show Group Preview
→ Join / Request to Join
```

---

# 31. Group Preview Before Join

Show:

```text
group name
avatar
member count
description
verification/security info if available
inviter
```

---

# 32. Join Modes

```rust
pub enum GroupJoinMode {
    Immediate,
    ApprovalRequired,
    InviteOnly,
}
```

---

# 33. Join Request

If approval required:

```text
Request to join
```

not:

```text
Join
```

---

# 34. Pending Join Request

Show:

```text
Request pending
Cancel request
```

---

# 35. Request Approval UX

Admins/moderators see:

```text
Join Requests
```

with:

```text
Approve
Decline
Block/Ban
```

---

# 36. Join Request Information

Show minimal safe context:

```text
identity
display name
verification
request time
```

---

# 37. Approve

Rust adds member.

---

# 38. Decline

Request removed.

---

# 39. Ban Requester

If moderation supports.

---

# 40. Membership Event

Timeline may show:

```text
Alice joined the group
```

---

# 41. Member List

Recommended row:

```text
avatar
name
role
verification/security warning if relevant
presence optionally
```

---

# 42. Member Sort

Recommended:

```text
Owner
Admins
Moderators
Members alphabetical
```

---

# 43. Large Group Member Search

Local search mandatory.

---

# 44. Member Detail

Actions depending capability:

```text
Message
Call
View Profile
Verify
Promote
Demote
Remove
Ban
```

---

# 45. Role Change UX

Example:

```text
Make Admin
Make Moderator
Remove Admin
```

---

# 46. Role Change Confirmation

Required for high-impact elevation.

Example:

```text
Make Alice an admin?
Admins can add/remove members and change group settings.
```

---

# 47. Demotion

Clear:

```text
Remove admin role
```

not vague:

```text
Change role
```

---

# 48. Owner Role

There should normally be exactly one owner.

---

# 49. Ownership Transfer

High-risk flow:

```text
Group Settings
→ Transfer Ownership
→ Select member
→ Explain consequences
→ Confirm
```

---

# 50. Ownership Transfer Confirmation

Strong confirmation.

Potential secondary authentication.

---

# 51. After Transfer

Old owner becomes:

```text
Admin
```

or product-defined role.

Must be explicit.

---

# 52. Owner Leaving

If owner attempts leave:

```text
transfer ownership first
```

unless backend supports auto-election.

Recommendation:

```text
require explicit transfer
```

for clarity.

---

# 53. Leave Group

Normal member action.

Confirmation:

```text
Leave this group?
```

Explain:

```text
You may lose access to future messages.
```

---

# 54. Leave + Local History

Separate option:

```text
Keep local history
Delete local history
```

if product supports.

---

# 55. Delete/Disband Group

Only if architecture supports owner destruction.

Must be distinct from:

```text
Leave Group
```

---

# 56. Disband Confirmation

Very high-risk.

Use explicit wording:

```text
Delete group for all members
```

---

# 57. Member Removal

Admin action.

```text
Remove Alice
```

---

# 58. Remove vs Ban

Remove:

```text
member leaves
may be invited again
```

Ban:

```text
cannot rejoin until unbanned
```

---

# 59. Ban

Requires clear explanation.

---

# 60. Banned List

Group settings:

```text
Banned Members
```

with:

```text
Unban
```

---

# 61. Removal Event

Timeline may show:

```text
Alice was removed by Bob
```

depending privacy/product design.

---

# 62. Ban Event

Can remain admin/audit-only if product chooses.

---

# 63. Moderation Log

Optional advanced feature.

Could show:

```text
role changes
removals
bans
invite revocations
```

---

# 64. Security/Audit Boundary

Security-important group events should come from Rust audit/event system.

---

# 65. Group Settings

Recommended categories:

```text
Group Info
Members
Roles & Permissions
Invites
Notifications
Shared Media
Security
Advanced
```

---

# 66. Group Name Edit

Capability-driven.

---

# 67. Group Avatar Edit

Same.

---

# 68. Group Description Edit

Same.

---

# 69. Group Permissions

Potential controls:

```text
Who can send messages?
Who can add members?
Who can edit group info?
Who can start calls?
```

---

# 70. Permission Profiles

Avoid huge ACL matrix initially.

Recommended presets:

```text
Standard Group
Admin-Only Posting
Managed
```

---

# 71. Custom Permissions

Future advanced option.

---

# 72. Read-Only Group

If only admins can post:

```text
Members can read but not send
```

Composer reflects this.

---

# 73. Role Capability Explanation

When user taps role:

```text
Admins can…
Moderators can…
```

---

# 74. Group Security Summary

Could show:

```text
End-to-end encrypted
Membership changed recently
Security needs attention
```

---

# 75. Membership Security Epoch

Backend handles epoch.

UI should not show:

```text
epoch 14
```

to normal users.

---

# 76. Membership Change Security

If adding/removing member causes rekey:

```text
no modal
```

unless failure.

---

# 77. Removed Member

Should not decrypt future messages.

UI simply shows membership result.

---

# 78. New Member History Visibility

Product decision:

```text
future messages only
or
selected history shared
```

Must be clear.

---

# 79. History Visibility Setting

If configurable:

```text
New members can see:
    From now on
    Recent history
    Full shared history
```

only if backend supports safely.

---

# 80. History Sharing Warning

Important privacy explanation.

---

# 81. Existing History

User should understand if new member can access old files/messages.

---

# 82. Group Verification

There is no simple single-person verification.

UX can show:

```text
group security
member identities
```

rather than "group verified" unless a true group identity model exists.

---

# 83. Group Security Warning

Examples:

```text
Member identity changed
Unknown member joined
Group key update failed
```

---

# 84. Security Warning Placement

Group header/details.

High-priority issue may show banner.

---

# 85. Member Identity Change

Show warning on affected member.

---

# 86. Group Member Profile

Links to Part 08 contact verification.

---

# 87. Contact vs Group Role

A member can be:

```text
Admin
```

but still:

```text
Unverified
```

These are independent.

---

# 88. Presence in Group

Do not show every member online state in large groups by default.

---

# 89. Small Group Presence

Member list may show presence.

---

# 90. Large Group Presence

Use:

```text
active participants
```

or no full presence list.

---

# 91. Typing in Group

Part 30.

Timeline shows:

```text
Alice is typing…
Alice and Bob are typing…
Several people are typing…
```

---

# 92. Read Receipts in Group

Small groups:

```text
Seen by 3
```

Large groups:

```text
aggregate count
```

or disabled depending policy.

---

# 93. Group Notification Settings

Per group:

```text
All messages
Mentions only
Muted
```

---

# 94. Mute Duration

Potential:

```text
1 hour
8 hours
1 week
Forever
```

---

# 95. Mention Override

Muted group may still notify on:

```text
@mention
```

if user chooses.

---

# 96. Group Call Entry

Header actions:

```text
Audio Call
Video Call
```

if supported.

---

# 97. Active Group Call Banner

If call active:

```text
3 people in call
Join
```

---

# 98. Group Call Participant Preview

Optional avatars.

---

# 99. Join Call

Rust validates:

```text
membership
call state
capacity
security
```

---

# 100. Group Call Role

Moderator/admin roles may have future call controls.

Not required v1.

---

# 101. Group Shared Media

Part 10 scoped view:

```text
Images
Videos
Documents
Audio
Links
```

---

# 102. Group Search

Part 11/32 scoped search.

---

# 103. Group Files

Large groups may accumulate much data.

Use pagination/virtualization.

---

# 104. Group Profile Desktop

Recommended:

```text
Main content:
    group header
    actions

Right inspector:
    members
    media
    settings
```

or full profile page.

---

# 105. Desktop Member Management

Wide table/list can show:

```text
Name
Role
Verification
Joined
Actions
```

---

# 106. Android Group Profile

Scrollable screen:

```text
Header
Action buttons
Members preview
Media preview
Notifications
Security
Leave
```

---

# 107. Android Member Management

Separate full-screen member list.

---

# 108. Tablet Group Profile

List/detail layout.

---

# 109. Member Action Bottom Sheet Android

```text
Message
View Profile
Change Role
Remove
Ban
```

---

# 110. Desktop Context Menu

Same semantic actions.

---

# 111. Member Selection for Removal/Role

Could support batch in desktop later.

Not necessary v1.

---

# 112. Group Creation Search

Search contacts locally.

---

# 113. Selected Member Chips

Android:

```text
horizontal chips/avatars
```

Desktop:

```text
selected list
```

---

# 114. Selection Accessibility

Announce count.

---

# 115. Group Name Validation

Rust authoritative.

Examples:

```text
empty
too long
invalid control chars
```

---

# 116. Avatar Upload

Uses same attachment/image pipeline concepts.

---

# 117. Group Invite Sharing

Android:

```text
share sheet
QR
copy
```

Desktop:

```text
copy
QR window
share/export
```

---

# 118. Invite Privacy

Invite code may grant access.

Warn:

```text
Anyone with this invite can request/join
```

depending semantics.

---

# 119. Invite Expiry

Display clearly.

---

# 120. Invite Usage Limit

If limited:

```text
3 uses remaining
```

---

# 121. Invite Regeneration

Revokes old if single active invite policy.

---

# 122. Join Request Notification

Admins can receive:

```text
New join request
```

if enabled.

---

# 123. Moderation Notification

High-frequency large group moderation events should not spam all members.

---

# 124. Role Change Notification

Affected user should see:

```text
You are now an admin
```

---

# 125. Demotion Notification

Likewise.

---

# 126. Removal Notification

Removed user:

```text
You were removed from the group
```

---

# 127. Ban Notification

Could show:

```text
You can no longer join this group
```

if product chooses.

---

# 128. Group Leave Notification

Other members may see timeline event.

---

# 129. Timeline Membership Events

Examples:

```text
Alice joined
Bob left
Carol became an admin
Group name changed
```

---

# 130. Event Density

Avoid flooding timeline in large groups.

Potential:

```text
collapse multiple membership events
```

---

# 131. Collapsed Event Example

```text
5 members joined
```

tap for details.

---

# 132. Large Group Scaling

Do not load:

```text
10,000 members
```

at once.

Use pagination/search.

---

# 133. Member Page

```rust
pub struct GroupMemberPage {
    pub members: Vec<GroupMemberView>,
    pub next_cursor: Option<GroupMemberCursor>,
}
```

---

# 134. Member Search

Backend/local index can search:

```text
name
role
verification
```

---

# 135. Lazy Member Presence

Only visible rows get presence overlay.

---

# 136. Group Join Request Paging

Likewise.

---

# 137. Banned List Paging

Likewise.

---

# 138. Role Counts

Group settings may show:

```text
1 Owner
3 Admins
2 Moderators
95 Members
```

---

# 139. Group Info Snapshot

```rust
pub struct GroupProfileView {
    pub summary: GroupSummaryView,
    pub description: Option<String>,
    pub my_capabilities: GroupCapabilities,
    pub member_preview: Vec<GroupMemberView>,
    pub notification_policy: GroupNotificationPolicy,
    pub security: GroupSecurityView,
}
```

---

# 140. Group Member View

```rust
pub struct GroupMemberView {
    pub account: AccountId,
    pub display_name: String,
    pub avatar: AvatarRef,
    pub role: GroupRole,
    pub verification: VerificationState,
    pub presence: Option<PresenceSummary>,
}
```

---

# 141. Group Security View

```rust
pub struct GroupSecurityView {
    pub encrypted: bool,
    pub warning: Option<GroupSecurityWarning>,
    pub recent_membership_change: bool,
}
```

---

# 142. Group Notification Policy

```rust
pub enum GroupNotificationPolicy {
    All,
    Mentions,
    Muted,
}
```

---

# 143. Group Join Request View

```rust
pub struct GroupJoinRequestView {
    pub request_id: GroupJoinRequestId,
    pub account: AccountId,
    pub display_name: String,
    pub verification: VerificationState,
    pub requested_at: Timestamp,
}
```

---

# 144. Role Assignment API

```rust
pub struct ChangeGroupRoleCommand {
    pub group: GroupId,
    pub member: AccountId,
    pub new_role: GroupRole,
}
```

---

# 145. Remove Member API

```rust
pub struct RemoveGroupMemberCommand {
    pub group: GroupId,
    pub member: AccountId,
    pub ban: bool,
}
```

---

# 146. Ownership Transfer API

```rust
pub struct TransferGroupOwnershipCommand {
    pub group: GroupId,
    pub new_owner: AccountId,
}
```

---

# 147. Group Presentation API

```rust
pub trait GroupPresentation {
    async fn profile(
        &self,
        group: GroupId,
    ) -> Result<GroupProfileView, UiError>;

    async fn members(
        &self,
        group: GroupId,
        cursor: Option<GroupMemberCursor>,
    ) -> Result<GroupMemberPage, UiError>;

    async fn add_members(
        &self,
        group: GroupId,
        members: Vec<AccountId>,
    ) -> Result<(), UiError>;

    async fn leave(
        &self,
        group: GroupId,
    ) -> Result<(), UiError>;

    async fn change_role(
        &self,
        command: ChangeGroupRoleCommand,
    ) -> Result<(), UiError>;

    async fn remove_member(
        &self,
        command: RemoveGroupMemberCommand,
    ) -> Result<(), UiError>;

    async fn transfer_ownership(
        &self,
        command: TransferGroupOwnershipCommand,
    ) -> Result<(), UiError>;
}
```

---

# 148. Invite Presentation API

```rust
pub trait GroupInvitePresentation {
    async fn create_invite(
        &self,
        group: GroupId,
        policy: GroupInvitePolicy,
    ) -> Result<GroupInviteView, UiError>;

    async fn revoke_invite(
        &self,
        group: GroupId,
        invite: GroupInviteId,
    ) -> Result<(), UiError>;

    async fn inspect_invite(
        &self,
        payload: GroupInvitePayload,
    ) -> Result<GroupInvitePreview, UiError>;

    async fn join(
        &self,
        payload: GroupInvitePayload,
    ) -> Result<GroupJoinResult, UiError>;
}
```

---

# 149. Join Request Presentation API

```rust
pub trait GroupJoinRequestPresentation {
    async fn list(
        &self,
        group: GroupId,
    ) -> Result<Vec<GroupJoinRequestView>, UiError>;

    async fn approve(
        &self,
        request: GroupJoinRequestId,
    ) -> Result<(), UiError>;

    async fn decline(
        &self,
        request: GroupJoinRequestId,
    ) -> Result<(), UiError>;
}
```

---

# 150. Group Events

```rust
pub enum GroupUiEvent {
    ProfileChanged(GroupProfileView),
    MemberAdded(GroupMemberView),
    MemberRemoved(AccountId),
    RoleChanged {
        member: AccountId,
        role: GroupRole,
    },
    JoinRequestAdded(GroupJoinRequestView),
    JoinRequestRemoved(GroupJoinRequestId),
    SecurityChanged(GroupSecurityView),
    NotificationPolicyChanged(GroupNotificationPolicy),
}
```

---

# 151. Android ViewModel

Owns:

```text
active section
search query
bottom sheet/dialog state
invite-share effect
confirmation UI
```

Rust owns group truth.

---

# 152. Dioxus Presenter

Owns:

```text
selection
inspector
member search
window/dialog state
keyboard focus
```

---

# 153. Capability-Driven UI

Hard rule:

```text
permissions come from Rust
```

Do not hardcode:

```text
if role == Admin → can remove
```

because managed policies may differ.

---

# 154. Optimistic Actions

Safe:

```text
mute notification setting
```

Potentially optimistic.

High-risk:

```text
remove member
change role
transfer ownership
```

should wait for Rust confirmation.

---

# 155. Role Change Race

If user's own admin permission is revoked during action:

```text
Rust rejects
UI refreshes
```

---

# 156. Member Removed Race

If target already left:

```text
show already no longer a member
```

---

# 157. Ownership Transfer Race

If target leaves before commit:

```text
fail safely
```

---

# 158. Offline Moderation

If backend supports queued admin commands:

```text
show pending
```

Otherwise require connection.

Recommendation:

```text
high-impact moderation commands require confirmed connectivity
```

unless CRDT/security semantics explicitly support offline.

---

# 159. Group Settings Offline

Local notification settings can update offline.

Membership authority actions may not.

---

# 160. Group Search Offline

Local member list/history search works for available data.

---

# 161. Group Profile Offline

Cached/local group info remains visible.

---

# 162. Group Security Offline

Show last known security state.

Do not claim current membership certainty if stale.

---

# 163. Membership Freshness

Usually not user-visible.

But if severe sync lag:

```text
Group membership may be out of date
```

advanced/degraded state.

---

# 164. Group Muted State

Sync account-wide if product policy.

---

# 165. Group Role Sync

Authoritative distributed state.

---

# 166. Multi-Device Membership

If user joins/leaves group on one device:

```text
other devices update
```

---

# 167. Leave on One Device

Usually account-level leave.

Other devices lose group membership.

---

# 168. Device-Local Group Archive

Archive may remain presentation preference.

Separate from membership.

---

# 169. Group Drafts

Per conversation/group via Part 06.

---

# 170. Group Mention Autocomplete

Composer uses member list/role data.

---

# 171. @Everyone

If supported:

```text
capability controlled
```

---

# 172. Abuse Prevention

Do not allow normal member to spam:

```text
role changes
invites
join requests
```

backend quotas/policies.

---

# 173. Invite Abuse

Show:

```text
Invite disabled
```

if rate/policy.

---

# 174. Join Flood

Admins should not receive hundreds of modal prompts.

Use paged request inbox.

---

# 175. Notification Flood

Join/leave events collapse.

---

# 176. Blocked Member in Group

Blocking a person does not necessarily remove them from group.

UI must explain distinction if both coexist.

---

# 177. Direct Block vs Group Membership

Possible policy:

```text
their group messages may remain visible
```

or hidden locally.

Must be explicit.

---

# 178. Report Group Member

If reporting exists:

```text
select messages
reason
submit
```

---

# 179. Group Report

Future moderation product.

Not core v1.

---

# 180. Accessibility — Member Row

Screen reader:

```text
Alice, Admin, verified, online
```

---

# 181. Role Change Accessibility

Confirmation explains resulting permissions.

---

# 182. Owner Transfer Accessibility

Must be completely operable with screen reader/keyboard.

---

# 183. Large Font

Member rows/settings must wrap.

---

# 184. RTL

Role and group layouts mirror correctly.

Group IDs/fingerprints retain canonical display direction where needed.

---

# 185. Color Independence

Owner/admin/moderator distinction needs:

```text
text
icon
```

not color only.

---

# 186. Reduced Motion

Member add/remove animation optional.

---

# 187. Keyboard Desktop

Examples:

```text
Ctrl/Cmd+F → search members
Enter → open selected member
Shift+F10 → member menu
```

---

# 188. Android TalkBack

Member action sheet exposes role/removal actions semantically.

---

# 189. Empty Members

Impossible for active group except transient corruption.

Handle safely.

---

# 190. Empty Join Requests

```text
No pending requests
```

---

# 191. Empty Banned List

```text
No banned members
```

---

# 192. Loading Large Member List

Use pagination.

---

# 193. Group Creation Empty Selection

Create disabled until minimum membership rules satisfied.

---

# 194. Group Name Missing

Inline error.

---

# 195. Invite Expired

```text
Invite expired
Ask an admin for a new invite
```

---

# 196. Invite Revoked

```text
This invite is no longer valid
```

---

# 197. Already Member

```text
You are already in this group
Open Group
```

---

# 198. Banned Join Attempt

Show:

```text
You cannot join this group
```

without exposing unnecessary moderation details.

---

# 199. Approval Pending

Persistent local state.

---

# 200. Approval Declined

Show:

```text
Join request declined
```

---

# 201. Group Deleted

If group disbanded:

```text
This group no longer exists
```

historical local messages may remain read-only according to policy.

---

# 202. Group Deleted While Open

Composer disables.

Timeline remains local history if retained.

---

# 203. Removed While Open

Immediately:

```text
composer disabled
"You were removed from this group"
```

---

# 204. Demoted While Editing Settings

Settings capability refreshes.

Unsaved UI form should not commit unauthorized change.

---

# 205. Group Rename While Open

Header updates.

---

# 206. Avatar Change

Updates across list/profile/timeline.

---

# 207. Group Call Begins

Optional banner appears.

---

# 208. Group Call Ends

Banner disappears.

---

# 209. Shared Media Update

No need to refresh full group profile.

Incremental section update.

---

# 210. Performance

Large group member list must be:

```text
paged
virtualized
searchable
```

---

# 211. Presence Update Granularity

Only visible member rows.

---

# 212. Role Change Granularity

Only target row/profile/counts.

---

# 213. Group Event Coalescing

Membership churn should not re-render full profile repeatedly.

---

# 214. Screenshot Test States

Required:

```text
group profile
member list
owner/admin/member
join request
invite preview
role confirmation
leave confirmation
removed state
banned list
security warning
active call banner
large group
dark mode
large font
RTL
```

---

# 215. Interaction Tests

Verify:

```text
create group
add member
remove member
change role
transfer ownership
leave
join via invite
approve request
mute
open media
start call
```

---

# 216. Multi-Device Tests

Join on phone:

```text
desktop updates
```

Role change on desktop:

```text
phone updates
```

Leave on one device:

```text
all devices reflect membership loss
```

---

# 217. Security Tests

Removed member cannot remain shown as active authorized member after Rust state updates.

---

# 218. Capability Tests

User without capability never gets enabled high-risk action.

---

# 219. Race Tests

```text
role revoked during action
member leaves during removal
owner target disappears
invite revoked while open
```

---

# 220. Accessibility Tests

Full ownership transfer and moderation flows usable without mouse/touch gesture.

---

# 221. Initial Production Recommendation

For v1, ship:

```text
private groups
group creation
member list
add members
invite QR/link
join
leave
owner/admin/member roles
role changes
member removal
block/ban if backend ready
group profile
shared media link
notification settings
active group call banner
membership timeline events
```

Defer:

```text
complex custom ACL matrix
public discovery
channels
broadcasts
large-scale moderator tooling
raised hands
event calendar
bots
advanced organization directory
```

---

# 222. Definition of Done

UI/UX Part 09 is complete when:

- group membership and authority come from Rust, not UI inference
- group roles and explicit capabilities are both represented
- group creation, add-member, invite, join, request, leave, removal, ban, and ownership transfer flows are defined
- ownership transfer is explicit and high-risk
- owner cannot accidentally leave without valid succession policy
- contact verification and group role remain separate concepts
- member identity warnings are visible without overwhelming normal UI
- small and large group behavior differ appropriately
- group calls, shared media, search, and notifications connect into the broader product
- desktop Dioxus and Android Compose management flows are platform-native
- member/join-request/banned lists are paged and virtualized
- role changes, removals, ownership transfer, and approval actions are race-safe and Rust-authoritative
- accessibility, keyboard/TalkBack, RTL, large font, and reduced motion are explicit
- the Rust Group/Invite/JoinRequest presentation contracts are defined
- offline, multi-device, stale invite, revoked permissions, removal, deletion, and large-group cases are tested

---

# 223. Final Architecture

```text
                    RUST GROUP CORE
                          │
       ┌──────────────────┼──────────────────┐
       │                  │                  │
   Membership          Roles            Invitations
       │                  │                  │
       └──────────────────┼──────────────────┘
                          │
                 Group Presentation
                    ┌─────┴─────┐
                    │           │
                 Dioxus      Compose
                    │           │
             Desktop Admin   Android Group UX
```

User-facing group model:

```text
Group
├── Members
├── Roles
├── Permissions
├── Invites
├── Requests
├── Security
├── Notifications
├── Shared Media
└── Calls
```

---

# 224. Final Principle

A group UI should make authority visible enough to prevent mistakes without turning the product into an ACL editor.

The right model is:

```text
clear roles
+
capability-driven actions
+
safe ownership/membership changes
+
understandable security state
+
platform-native management flows
```

not:

```text
UI guesses what an admin can do
```

This keeps group governance predictable across Dioxus desktop and Android Compose while the Rust group/security core remains authoritative.
