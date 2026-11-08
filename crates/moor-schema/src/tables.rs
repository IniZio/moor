//! The store's tables and columns as `sea-query` identifiers, so a query names a column the
//! compiler checks. A test holds them to the schema the migrations build.
//!
//! The migrations keep their own names: an applied one is history, so a rename here must not
//! change what it creates.

use sea_query::Iden;

/// One enum per table: `Table` names the table, every other variant one of its columns, in
/// table order.
macro_rules! tables {
    ($($table:ident { $($col:ident),+ $(,)? })+) => {
        $(
            #[doc = concat!("The `", stringify!($table), "` table and its columns.")]
            #[allow(missing_docs, reason = "each variant is the column of its name")]
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Iden)]
            pub enum $table {
                Table,
                $($col),+
            }
        )+

        /// Every table with its column names in table order. A table comes after every table its
        /// foreign keys reference (a test holds the list to that), so rows inserted in this order
        /// never break a key.
        #[must_use]
        pub fn all() -> Vec<(String, Vec<String>)> {
            vec![$((
                $table::Table.to_string(),
                vec![$($table::$col.to_string()),+],
            )),+]
        }
    };
}

tables! {
    Meta { K, V }
    Users { Id, CreatedAt, Email }
    Identities { Identity, User, LinkedAt }
    UserSecrets { User, Name, Value, UpdatedAt }
    FrontLeases { TokenHash, Front, User, ExpiresAt }
    UserMcpServers { User, Name, Url, Header, Secret, CreatedAt, UpdatedAt }
    UserMcpOauth { User, Name, Value, ExpiresAt, UpdatedAt, SignedOutAt }
    McpOauthPending { State, User, Name, Value, ExpiresAt }
    UserProfiles { User, Repo, GitRef, UpdatedAt }
    UserWebhooks { Id, User, Url, Events, Key, CreatedAt, LastOkAt, FailingSince, LastError, State,
        Header, HeaderValue }
    Boxes { Id, Name, Repo, RepoConfig, AuthorName, AuthorEmail, User, Issue, Prompt, Base, Branch,
        State, StateReason, IntentKind, IntentPid, IntentRun, IntentAt, SetupFailedAt, IdleSince,
        LaunchFailure, Provider, Handle, DriverState, AttemptedHandle, Workdir, Egress,
        SleepReason, Cpus, MemMib, DriverUpdatedAt, CreatedAt, UpdatedAt, Node, NodeLabels,
        NodeMemMib, ResumeFirstAt, OtherRepos, StepsPendingAt, SuspendAhead, SuspendDirty,
        SuspendCheckedAt, HeldPrompt, SuspendPublish }
    Sessions { Id, Box, Ordinal, Agent, Model, AgentSessionId, WorkspaceId, Pane, State, StartedAt,
        EndedAt, ParentSession, Workdir, User, HostKey }
    Launches { Id, Box, SessionRef, Pane, StartedAt, EndedAt, Exit, TranscriptDigest,
        RegisteredAgentSession, Session, Holder, AgentReadAt, AskedAt }
    BoxRefs { Id, Box, Kind, Value }
    Events { Id, Box, Session, Kind, Payload, Ts }
    Runs { Id, Box, Argv, Record, Sha, Pid, StartedAt, FinishedAt, Exit, Interrupted }
    Reports { Id, Box, Session, Kind, Version, Path, Title, CreatedAt }
    Proxy { Box, BoxPort, EgressPort, Pid, Handle, Ready, LeaseOwner, LeaseHeartbeat, BootId,
        UpdatedAt, KeyHash, Ledger, Spec, CaCert, CaKey, TokenHash, TokenDelivered,
        ForwardPort }
    BoxLease { Id, Box, Kind, Key, State, Booted, Pgid, Sid, RootPid, RootStart, SizeAtRelease,
        CreatedAt, UpdatedAt }
    Artifacts { Id, Box, Session, Name, Version, Kind, ContentType, Size, Sha256, Path, CreatedAt }
    ArtifactCursors { Box, Through }
    Bindings { Id, Front, ConvKey, Session, Owner, CreatedAt }
    BoxProfiles { Box, Repo, Sha, Manifest }
    WebhookDeliveries { Id, Webhook, Event, LeasedUntil }
    UserMachinesV2 { MachineId, User, Name, Pubkey, Labels, Capacity, EnrolledAt, PoolProvider }
    MachineLinks { MachineId, Online, Since, Version, Protocol, MaxBoxes, CacheGb, PoolFeatures,
        MemMib, Cpus, ReserveMib, MemAvailableMib, PsiCenti, ReportedAt, PsiFullCenti, DiskFreeMib,
        DiskTotalMib, RunnerAddr, HeardAt, DrainingSince, WarmStarts }
    BoxMachines { Box, User, State, Machine, Labels, Ports, Slot, SyncExclude, Since, Until,
        Enrolled }
    NodePressure { Node, Readings, HighSince, SuspendedAt }
    MachineBoxMemory { MachineId, BoxId, HostMemoryMib, ResumeMemoryMib }
    MachineBoxes { BoxId, MachineId, Slot, Reach, Facts, Prepared, SuspendedAt, Evicted,
        UpdatedAt }
    SetupCheckpoints { Provider, LayerKey, Checkpoint, Repo, Bytes, CreatedAt, LastUsedAt, Workdir,
        SetupMs, User, Chain, ChainAt, Parent, Evicting, Base, Depth, StaleSince }
    SetupCheckpointPins { BoxId, Provider, LayerKey, Layer, Checkpoint }
    Workflows { Id, User, Name, Cron, Timezone, Repos, Prompt, EnabledAt, DisabledReason,
        TimeoutMinutes, CreatedAt, UpdatedAt, OriginFront, OriginKey, ChangeSeq }
    WorkflowRuns { Id, Workflow, Repo, DueAt, Box, Outcome, Detail, EndedAt,
        RecordedAt, Seq }
    WorkflowSeq { Id, N }
}
