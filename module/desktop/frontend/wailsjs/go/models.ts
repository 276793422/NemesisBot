export namespace main {
	
	export class ApprovalHistoryItem {
	    request_id: string;
	    operation: string;
	    operation_name: string;
	    target: string;
	    risk_level: string;
	    approved: boolean;
	    timed_out: boolean;
	    duration_seconds: number;
	    response_time: number;
	    timestamp: number;
	
	    static createFrom(source: any = {}) {
	        return new ApprovalHistoryItem(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.request_id = source["request_id"];
	        this.operation = source["operation"];
	        this.operation_name = source["operation_name"];
	        this.target = source["target"];
	        this.risk_level = source["risk_level"];
	        this.approved = source["approved"];
	        this.timed_out = source["timed_out"];
	        this.duration_seconds = source["duration_seconds"];
	        this.response_time = source["response_time"];
	        this.timestamp = source["timestamp"];
	    }
	}
	export class ApprovalRequest {
	    request_id: string;
	    operation: string;
	    operation_name: string;
	    target: string;
	    risk_level: string;
	    reason: string;
	    timeout_seconds: number;
	    context: Record<string, string>;
	    timestamp: number;
	
	    static createFrom(source: any = {}) {
	        return new ApprovalRequest(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.request_id = source["request_id"];
	        this.operation = source["operation"];
	        this.operation_name = source["operation_name"];
	        this.target = source["target"];
	        this.risk_level = source["risk_level"];
	        this.reason = source["reason"];
	        this.timeout_seconds = source["timeout_seconds"];
	        this.context = source["context"];
	        this.timestamp = source["timestamp"];
	    }
	}
	export class ApprovalResponse {
	    request_id: string;
	    approved: boolean;
	    timed_out: boolean;
	    duration_seconds: number;
	    response_time: number;
	
	    static createFrom(source: any = {}) {
	        return new ApprovalResponse(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.request_id = source["request_id"];
	        this.approved = source["approved"];
	        this.timed_out = source["timed_out"];
	        this.duration_seconds = source["duration_seconds"];
	        this.response_time = source["response_time"];
	    }
	}
	export class ApprovalStats {
	    total_requests: number;
	    approved: number;
	    denied: number;
	    timeout: number;
	    avg_duration: number;
	
	    static createFrom(source: any = {}) {
	        return new ApprovalStats(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.total_requests = source["total_requests"];
	        this.approved = source["approved"];
	        this.denied = source["denied"];
	        this.timeout = source["timeout"];
	        this.avg_duration = source["avg_duration"];
	    }
	}
	export class ChatMessage {
	    id: string;
	    role: string;
	    content: string;
	    timestamp: number;
	
	    static createFrom(source: any = {}) {
	        return new ChatMessage(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.id = source["id"];
	        this.role = source["role"];
	        this.content = source["content"];
	        this.timestamp = source["timestamp"];
	    }
	}
	export class DesktopInfo {
	    version: string;
	    environment: string;
	    bot_state: string;
	    uptime: string;
	
	    static createFrom(source: any = {}) {
	        return new DesktopInfo(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.version = source["version"];
	        this.environment = source["environment"];
	        this.bot_state = source["bot_state"];
	        this.uptime = source["uptime"];
	    }
	}
	export class KeyboardShortcut {
	    key: string;
	    action: string;
	    description: string;
	
	    static createFrom(source: any = {}) {
	        return new KeyboardShortcut(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.key = source["key"];
	        this.action = source["action"];
	        this.description = source["description"];
	    }
	}
	export class LogEntry {
	    timestamp: string;
	    level: string;
	    module: string;
	    message: string;
	
	    static createFrom(source: any = {}) {
	        return new LogEntry(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.timestamp = source["timestamp"];
	        this.level = source["level"];
	        this.module = source["module"];
	        this.message = source["message"];
	    }
	}
	export class Setting {
	    key: string;
	    value: string;
	    type: string;
	    description: string;
	
	    static createFrom(source: any = {}) {
	        return new Setting(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.key = source["key"];
	        this.value = source["value"];
	        this.type = source["type"];
	        this.description = source["description"];
	    }
	}
	export class SimulatedRequest {
	    id: string;
	    name: string;
	    description: string;
	    operation: string;
	    operation_name: string;
	    example_target: string;
	    risk_level: string;
	
	    static createFrom(source: any = {}) {
	        return new SimulatedRequest(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.id = source["id"];
	        this.name = source["name"];
	        this.description = source["description"];
	        this.operation = source["operation"];
	        this.operation_name = source["operation_name"];
	        this.example_target = source["example_target"];
	        this.risk_level = source["risk_level"];
	    }
	}
	export class SystemStatus {
	    uptime: string;
	    memory_usage_mb: number;
	    cpu_usage_percent: number;
	    thread_count: number;
	    version: string;
	    go_version: string;
	
	    static createFrom(source: any = {}) {
	        return new SystemStatus(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.uptime = source["uptime"];
	        this.memory_usage_mb = source["memory_usage_mb"];
	        this.cpu_usage_percent = source["cpu_usage_percent"];
	        this.thread_count = source["thread_count"];
	        this.version = source["version"];
	        this.go_version = source["go_version"];
	    }
	}
	export class ThemeConfig {
	    current_theme: string;
	    auto_theme: boolean;
	
	    static createFrom(source: any = {}) {
	        return new ThemeConfig(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.current_theme = source["current_theme"];
	        this.auto_theme = source["auto_theme"];
	    }
	}

}

