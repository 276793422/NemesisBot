export namespace main {
	
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

}

