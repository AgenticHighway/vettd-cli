pub(crate) fn shared_signal_weight(signal: &str) -> Option<i32> {
    match signal {
        "credential_exposure_signal" => Some(25),
        "dangerous_combo:shell+network+fs" => Some(30),
        "dangerous_keyword:exfiltrate"
        | "dangerous_keyword:reverse"
        | "dangerous_keyword:steal" => Some(35),
        "dangerous_keyword:wipe" => Some(30),
        "dangerous_keyword:bypass" => Some(25),
        "keyword:shell" => Some(15),
        "keyword:browser" | "keyword:api" | "keyword:network" => Some(10),
        "keyword:execute" => Some(12),
        "keyword:filesystem" => Some(5),
        _ => None,
    }
}

pub(crate) fn secret_signal_weight(signal: &str) -> Option<i32> {
    match signal {
        "secret:aws:access_key"
        | "secret:aws:secret_access_key"
        | "secret:aws:session_token"
        | "secret:github:pat"
        | "secret:github:oauth_token"
        | "secret:github:fine_grained_pat"
        | "secret:github:app_token"
        | "secret:github:refresh_token"
        | "secret:gitlab:pat"
        | "secret:gitlab:project_token"
        | "secret:gitlab:oauth_token"
        | "secret:stripe:live_secret_key"
        | "secret:stripe:restricted_key"
        | "secret:npm:token"
        | "secret:pypi:token" => Some(25),
        "secret:gcp:api_key"
        | "secret:gcp:client_secret"
        | "secret:azure:account_key"
        | "secret:azure:connection_string"
        | "secret:azure:secret_value"
        | "secret:azure:sas_token"
        | "secret:slack:bot_token"
        | "secret:slack:user_token"
        | "secret:slack:webhook"
        | "secret:twilio:auth_token"
        | "secret:twilio:api_key"
        | "secret:sendgrid:api_key"
        | "secret:mailgun:api_key"
        | "secret:auth:basic_header"
        | "secret:auth:bearer_header" => Some(20),
        "secret:stripe:test_secret_key" | "secret:auth:jwt" => Some(15),
        _ if signal.starts_with("secret:crypto:") => Some(25),
        _ => None,
    }
}

pub(crate) fn ssrf_signal_weight(signal: &str) -> Option<i32> {
    match signal {
        "ssrf:metadata:aws"
        | "ssrf:metadata:gcp"
        | "ssrf:metadata:azure"
        | "ssrf:metadata:alibaba"
        | "ssrf:scheme:gopher" => Some(45),
        "ssrf:scheme:file"
        | "ssrf:scheme:dict"
        | "ssrf:encoding:octal_ipv4"
        | "ssrf:encoding:hex_ipv4"
        | "ssrf:encoding:decimal_host" => Some(25),
        "ssrf:private_network:10"
        | "ssrf:private_network:172"
        | "ssrf:private_network:192"
        | "ssrf:private_network:localhost" => Some(20),
        _ => None,
    }
}

pub(crate) fn common_cognitive_signal_weight(signal: &str) -> Option<i32> {
    match signal {
        "cognitive_tampering:role_override" | "cognitive_tampering:delimiter_framing" => Some(45),
        "cognitive_tampering:instruction_injection"
        | "cognitive_tampering:unicode_steganography" => Some(35),
        "cognitive_tampering:base64_encoded" => Some(25),
        _ => None,
    }
}
