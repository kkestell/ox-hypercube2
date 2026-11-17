PRAGMA user_version=2;
PRAGMA foreign_keys=OFF;
BEGIN TRANSACTION;
CREATE TABLE workspaces (
            id   TEXT PRIMARY KEY,
            path TEXT NOT NULL UNIQUE
        );
INSERT INTO workspaces VALUES('w','/work');
CREATE TABLE worktrees (
            id             TEXT PRIMARY KEY,
            path           TEXT NOT NULL,
            branch         TEXT NOT NULL,
            base           TEXT NOT NULL,
            created_branch INTEGER NOT NULL,
            common_dir     TEXT NOT NULL,
            compare        TEXT
        );
INSERT INTO worktrees VALUES('0a1b2c3d','/worktrees/work-0a1b2c3d','ox/0a1b2c3d','abc',1,'/repos/work/.git','main');
CREATE TABLE sessions (
            number           INTEGER PRIMARY KEY AUTOINCREMENT,
            id               TEXT NOT NULL UNIQUE,
            workspace_id     TEXT NOT NULL REFERENCES workspaces(id),
            command          TEXT NOT NULL,  -- JSON array
            cwd              TEXT NOT NULL,
            title            TEXT,
            agent_session_id TEXT,
            worktree_id      TEXT REFERENCES worktrees(id) ON DELETE SET NULL,
            created_at       INTEGER NOT NULL
        , config TEXT NOT NULL DEFAULT '[]');
INSERT INTO sessions VALUES(1,'plain','w','["claude-code-acp"]','/work','Plain session','agent-plain',NULL,1700000000000,'[{"config_id":"model","value":"claude-sonnet-5-5"}]');
INSERT INTO sessions VALUES(2,'archived','w','["pi-acp"]','/worktrees/work-0a1b2c3d','Archived session','agent-archived','0a1b2c3d',1700000000100,'[]');
CREATE TABLE events (
            session_id  TEXT    NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            seq         INTEGER NOT NULL,
            recorded_at INTEGER NOT NULL,
            kind        TEXT    NOT NULL,  -- JSON of EventKind
            PRIMARY KEY (session_id, seq)
        ) WITHOUT ROWID;
INSERT INTO events VALUES('archived',1,1700000001001,'{"type":"prompt_started","data":{"prompt":[{"type":"text","text":"Fix the build"}]}}');
INSERT INTO events VALUES('archived',2,1700000001002,'{"type":"agent_parked"}');
INSERT INTO events VALUES('plain',1,1700000001001,'{"type":"prompt_started","data":{"prompt":[{"type":"text","text":"Add a test"}]}}');
INSERT INTO events VALUES('plain',2,1700000001002,'{"type":"agent_started","data":{"agent_session_id":"agent-plain","prompt_capabilities":{"image":{},"embeddedContext":{}},"config_options":[{"configId":"model","name":"Model","category":"model","type":"select","currentValue":"claude-opus-5-5","options":[{"value":"claude-opus-5-5","name":"Opus 5.5"},{"value":"claude-sonnet-5-5","name":"Sonnet 5.5"}]},{"configId":"mode","name":"Mode","category":"mode","type":"select","currentValue":"auto","options":[{"value":"auto","name":"Auto"}]}],"available_commands":[{"type":"command","name":"review","description":"Review the change","input":{"type":"text","hint":"[target]"}}]}}');
INSERT INTO events VALUES('plain',3,1700000001003,'{"type":"update","data":{"sessionUpdate":"user_message_chunk","messageId":"m0","content":{"type":"text","text":"Add a test"}}}');
INSERT INTO events VALUES('plain',4,1700000001004,'{"type":"update","data":{"sessionUpdate":"user_message","messageId":"m0","content":[{"type":"text","text":"Add a test"}]}}');
INSERT INTO events VALUES('plain',5,1700000001005,'{"type":"update","data":{"sessionUpdate":"state_update","state":"running","background":[]}}');
INSERT INTO events VALUES('plain',6,1700000001006,'{"type":"update","data":{"sessionUpdate":"agent_thought_chunk","messageId":"m1","content":{"type":"text","text":"Reading"}}}');
INSERT INTO events VALUES('plain',7,1700000001007,'{"type":"update","data":{"sessionUpdate":"agent_thought","messageId":"m1","content":[{"type":"text","text":"Reading the code first."}]}}');
INSERT INTO events VALUES('plain',8,1700000001008,'{"type":"update","data":{"sessionUpdate":"agent_message_chunk","messageId":"m2","content":{"type":"text","text":"Looking"}}}');
INSERT INTO events VALUES('plain',9,1700000001009,'{"type":"update","data":{"sessionUpdate":"agent_message","messageId":"m2","content":[{"type":"text","text":"Looking at the parser."}]}}');
INSERT INTO events VALUES('plain',10,1700000001010,'{"type":"update","data":{"sessionUpdate":"tool_call_update","toolCallId":"tool-1","name":"Bash","title":"Bash `cargo test`","kind":"execute","status":"pending","rawInput":{"command":"cargo test","description":"Run the tests"}}}');
INSERT INTO events VALUES('plain',11,1700000001011,'{"type":"update","data":{"sessionUpdate":"tool_call_update","toolCallId":"tool-1","status":"in_progress"}}');
INSERT INTO events VALUES('plain',12,1700000001012,'{"type":"update","data":{"sessionUpdate":"tool_call_update","toolCallId":"tool-1","status":"completed","content":[{"type":"content","content":{"type":"text","text":"test result: ok"}}]}}');
INSERT INTO events VALUES('plain',13,1700000001013,'{"type":"prompt_steered","data":{"prompt":[{"type":"text","text":"Also the docs"}],"from":{"session_id":"other","title":"Other session"}}}');
INSERT INTO events VALUES('plain',14,1700000001014,'{"type":"steered_prompt_taken","data":{"prompt":13}}');
INSERT INTO events VALUES('plain',15,1700000001015,'{"type":"prompt_steered","data":{"prompt":[{"type":"text","text":"Never mind"}]}}');
INSERT INTO events VALUES('plain',16,1700000001016,'{"type":"steered_prompt_dropped","data":{"prompt":15}}');
INSERT INTO events VALUES('plain',17,1700000001017,'{"type":"permission_requested","data":{"request_id":"1","title":"Run cargo test?","description":null,"subject":null,"options":[{"optionId":"allow","name":"Allow","kind":"allow_once"},{"optionId":"reject","name":"Reject","kind":"reject_once"}]}}');
INSERT INTO events VALUES('plain',18,1700000001018,'{"type":"permission_resolved","data":{"request_id":"1","outcome":{"outcome":"selected","optionId":"allow"}}}');
INSERT INTO events VALUES('plain',19,1700000001019,'{"type":"permission_requested","data":{"request_id":"2","title":"Start a session in work?","description":null,"subject":null,"options":[{"optionId":"start","name":"Start Session","kind":"allow_once"},{"optionId":"decline","name":"Decline","kind":"reject_once"}],"session_request":{"harness":"claude","model":"Opus 5.5","provider":null,"effort":"Medium","mode":"Auto","checkout":{"type":"worktree","branch":null,"base":"main"},"prompt":"Write the docs"}}}');
INSERT INTO events VALUES('plain',20,1700000001020,'{"type":"permission_resolved","data":{"request_id":"2","outcome":{"outcome":"cancelled"}}}');
INSERT INTO events VALUES('plain',21,1700000001021,'{"type":"elicitation_requested","data":{"request_id":"3","message":"Claude has a question","tool_call_id":"tool-2","requested_schema":{"type":"object","properties":{"q1":{"type":"string","title":"Scope","description":"Which tests?","oneOf":[{"const":"Unit","title":"Unit"},{"const":"All","title":"All"}]}},"required":["q1"]}}}');
INSERT INTO events VALUES('plain',22,1700000001022,'{"type":"elicitation_resolved","data":{"request_id":"3","action":{"action":"accept","content":{"q1":"Unit"}}}}');
INSERT INTO events VALUES('plain',23,1700000001023,'{"type":"update","data":{"sessionUpdate":"available_commands_update","availableCommands":[{"type":"command","name":"review","description":"Review the change","input":{"type":"text","hint":"[target]"}},{"type":"command","name":"compact","description":"Compact the conversation"}]}}');
INSERT INTO events VALUES('plain',24,1700000001024,'{"type":"update","data":{"sessionUpdate":"config_option_update","configOptions":[{"configId":"model","name":"Model","category":"model","type":"select","currentValue":"claude-sonnet-5-5","options":[{"value":"claude-opus-5-5","name":"Opus 5.5"},{"value":"claude-sonnet-5-5","name":"Sonnet 5.5"}]}]}}');
INSERT INTO events VALUES('plain',25,1700000001025,'{"type":"update","data":{"sessionUpdate":"session_info_update","title":"Add a parser test"}}');
INSERT INTO events VALUES('plain',26,1700000001026,'{"type":"update","data":{"sessionUpdate":"state_update","state":"idle","stopReason":"end_turn","background":[]}}');
INSERT INTO events VALUES('plain',27,1700000001027,'{"type":"turn_ended","data":{"stop_reason":{"stopReason":"end_turn"},"error":null}}');
INSERT INTO events VALUES('plain',28,1700000001028,'{"type":"turn_started"}');
INSERT INTO events VALUES('plain',29,1700000001029,'{"type":"turn_ended","data":{"stop_reason":null,"error":"the agent exited"}}');
INSERT INTO events VALUES('plain',30,1700000001030,'{"type":"agent_exited","data":{"error":null}}');
INSERT INTO events VALUES('plain',31,1700000001031,'{"type":"agent_parked"}');
INSERT INTO events VALUES('plain',32,1700000001032,'{"type":"session_closed"}');
CREATE TABLE archived (
            session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE
        );
INSERT INTO archived VALUES('archived');
CREATE TABLE session_unseen (
            session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
            unseen     TEXT NOT NULL
        );
INSERT INTO session_unseen VALUES('plain','finished');
CREATE TABLE session_queues (
            session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
            paused     INTEGER NOT NULL,
            prompts    TEXT NOT NULL  -- JSON array of QueuedPrompt
        );
INSERT INTO session_queues VALUES('plain',1,'[{"id":"q1","prompt":[{"type":"text","text":"Then run the tests"}]}]');
CREATE TABLE deleted_agent_sessions (
            agent_session_id TEXT PRIMARY KEY
        );
INSERT INTO deleted_agent_sessions VALUES('agent-deleted');
INSERT INTO sqlite_sequence VALUES('sessions',3);
COMMIT;
