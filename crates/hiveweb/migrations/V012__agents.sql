CREATE TABLE agents (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    identifier VARCHAR(64) NOT NULL UNIQUE COMMENT '入口固定为 "main"',
    name VARCHAR(128) NOT NULL,
    description VARCHAR(512) NULL,
    system_prompt TEXT NOT NULL,
    parent_agent_id BIGINT NULL,
    depth TINYINT NOT NULL DEFAULT 0 COMMENT '深度，main = 0，子 = parent.depth+1',
    model_preset VARCHAR(64) NULL COMMENT 'hiveweb llm_presets.toml 中命名 preset；NULL = 全局默认',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    INDEX idx_agents_parent (parent_agent_id),
    INDEX idx_agents_model_preset (model_preset),
    CONSTRAINT fk_agents_parent FOREIGN KEY (parent_agent_id) REFERENCES agents(id) ON DELETE RESTRICT,
    CONSTRAINT chk_agents_depth CHECK (depth >= 0 AND depth <= 10)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE agent_tools (
    agent_id BIGINT NOT NULL,
    tool_id BIGINT NOT NULL,
    PRIMARY KEY (agent_id, tool_id),
    CONSTRAINT fk_at_agent FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE CASCADE,
    CONSTRAINT fk_at_tool  FOREIGN KEY (tool_id)  REFERENCES tools(id)  ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE agent_skills (
    agent_id BIGINT NOT NULL,
    skill_id BIGINT NOT NULL,
    PRIMARY KEY (agent_id, skill_id),
    CONSTRAINT fk_as_agent FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE CASCADE,
    CONSTRAINT fk_as_skill FOREIGN KEY (skill_id) REFERENCES skills(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE agent_permissions (
    agent_id BIGINT NOT NULL,
    capability VARCHAR(64) NOT NULL,
    PRIMARY KEY (agent_id, capability),
    CONSTRAINT fk_ap_agent FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
