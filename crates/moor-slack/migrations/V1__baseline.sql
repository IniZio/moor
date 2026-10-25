CREATE TABLE slack_events (event_id TEXT PRIMARY KEY, seen_at INTEGER NOT NULL);
CREATE TABLE slack_threads (
    channel TEXT NOT NULL,
    thread_ts TEXT NOT NULL,
    owner TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    workdir TEXT,
    root_session INTEGER,
    last_turn_at INTEGER,
    PRIMARY KEY (channel, thread_ts)
);
CREATE TABLE slack_boxes (
    box_name TEXT PRIMARY KEY,
    channel TEXT NOT NULL,
    thread_ts TEXT NOT NULL,
    n INTEGER NOT NULL,
    repo TEXT NOT NULL,
    goal TEXT NOT NULL,
    base TEXT,
    parent INTEGER,
    requested_by TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('starting', 'bound', 'closing', 'gone')),
    box_id INTEGER,
    branch TEXT,
    status_ts TEXT,
    status_text TEXT,
    discard INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    UNIQUE (channel, thread_ts, n)
);
CREATE INDEX slack_boxes_box ON slack_boxes (box_id);
CREATE TABLE slack_inbox (
    id INTEGER PRIMARY KEY,
    channel TEXT NOT NULL,
    thread_ts TEXT NOT NULL,
    message_ts TEXT,
    kind TEXT NOT NULL CHECK (kind IN ('message', 'notice', 'report')),
    author TEXT CHECK (kind != 'message' OR author IS NOT NULL),
    text TEXT NOT NULL,
    source TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL,
    turn_id INTEGER,
    done_at INTEGER
);
CREATE INDEX slack_inbox_thread ON slack_inbox (channel, thread_ts, id);
CREATE TABLE slack_turns (
    id INTEGER PRIMARY KEY,
    channel TEXT NOT NULL,
    thread_ts TEXT NOT NULL,
    paid_by TEXT NOT NULL,
    reacted_ts TEXT,
    rerun_of INTEGER,
    started_at INTEGER NOT NULL,
    pid INTEGER,
    pid_started INTEGER,
    ended_at INTEGER,
    end TEXT CHECK (end IN
        ('replied', 'failed', 'killed', 'unsafe', 'lost', 'no_token', 'token_refused')),
    reply_text TEXT,
    posted_at INTEGER,
    context_bytes INTEGER,
    cost_usd REAL
);
CREATE INDEX slack_turns_thread ON slack_turns (channel, thread_ts, id);
CREATE TABLE slack_actions (
    turn_id INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    tool TEXT NOT NULL,
    box_name TEXT NOT NULL,
    requested_by TEXT NOT NULL,
    args TEXT NOT NULL,
    outcome TEXT,
    created_at INTEGER NOT NULL,
    done_at INTEGER,
    PRIMARY KEY (turn_id, seq)
);
CREATE TABLE slack_channel_repos (
    channel TEXT NOT NULL,
    repo TEXT NOT NULL,
    added_by TEXT NOT NULL,
    added_at INTEGER NOT NULL,
    is_default INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0, 1)),
    PRIMARY KEY (channel, repo)
);
CREATE UNIQUE INDEX slack_channel_repos_one_default
    ON slack_channel_repos (channel) WHERE is_default = 1;
CREATE TABLE slack_prompts (
    id INTEGER PRIMARY KEY,
    box_name TEXT NOT NULL,
    event_id INTEGER NOT NULL,
    message_ts TEXT CHECK (message_ts IS NOT NULL OR closed_reason IS NOT NULL),
    question TEXT NOT NULL,
    options TEXT NOT NULL,
    answer TEXT,
    answered_by TEXT CHECK ((answer IS NULL) = (answered_by IS NULL)),
    closed_reason TEXT CHECK (answer IS NULL OR closed_reason IS NULL),
    answer_sent INTEGER NOT NULL DEFAULT 0 CHECK (answer_sent = 0 OR answer IS NOT NULL),
    shown_at INTEGER,
    created_at INTEGER NOT NULL,
    UNIQUE (box_name, event_id)
);
CREATE TABLE slack_artifacts (
    box_name TEXT NOT NULL,
    name TEXT NOT NULL,
    version INTEGER NOT NULL,
    failed_at INTEGER,
    tried_at INTEGER,
    tries INTEGER NOT NULL DEFAULT 0,
    told INTEGER NOT NULL DEFAULT 0,
    posted_at INTEGER CHECK (posted_at IS NOT NULL OR failed_at IS NOT NULL),
    gave_up_at INTEGER CHECK (gave_up_at IS NULL OR (failed_at IS NOT NULL AND posted_at IS NULL)),
    PRIMARY KEY (box_name, name, version)
);
