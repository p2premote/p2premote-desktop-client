use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedInviteInfo {
    pub device_code: String,
    pub temporary_password: String,
}

pub fn parse_invite_info(input: &str) -> Option<ParsedInviteInfo> {
    let mut device_code = None;
    let mut temporary_password = None;

    for line in input.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Some((key, value)) = line.split_once('：').or_else(|| line.split_once(':')) else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        if key.contains("设备代码") || key == "code" || key == "device code" {
            device_code = normalize_token(value);
        } else if key.contains("临时密码")
            || key == "password"
            || key == "temporary password"
        {
            temporary_password = normalize_token(value);
        }
    }

    if device_code.is_none() || temporary_password.is_none() {
        let tokens = input
            .split_whitespace()
            .filter_map(normalize_token)
            .collect::<Vec<_>>();
        if tokens.len() == 2 {
            device_code = Some(tokens[0].clone());
            temporary_password = Some(tokens[1].clone());
        }
    }

    let device_code = device_code?;
    let temporary_password = temporary_password?;
    (device_code.len() >= 6 && temporary_password.len() >= 4).then_some(ParsedInviteInfo {
        device_code,
        temporary_password,
    })
}

fn normalize_token(value: &str) -> Option<String> {
    let value = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gui_invites_in_both_languages() {
        let english = parse_invite_info("Device code: AB12-CD34\nTemporary password: xy 9876")
            .unwrap();
        assert_eq!(english.device_code, "AB12CD34");
        assert_eq!(english.temporary_password, "xy9876");

        let chinese = parse_invite_info("设备代码：AB12 CD34\n临时密码：xy-9876").unwrap();
        assert_eq!(chinese, english);
    }

    #[test]
    fn rejects_labels_as_credentials_and_short_values() {
        assert!(parse_invite_info("Device code ABC Temporary password DEF").is_none());
        assert!(parse_invite_info("ABC DEF").is_none());
        assert!(parse_invite_info("ABCDEF 1234").is_some());
    }
}
