use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    Option::deserialize(d)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub approve_mode: Option<Approval>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub verification_method: Option<Verification>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Approval {
    Password,
    Click,
    PasswordClick,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verification {
    #[serde(rename = "use-temporary-password")]
    Temporary,
    #[serde(rename = "use-permanent-password")]
    Permanent,
    #[serde(rename = "use-both-passwords")]
    Both,
}

impl Settings {
    pub fn from_options(
        options: &std::collections::HashMap<String, String>,
    ) -> Result<Option<Self>, serde_json::Error> {
        let values: BTreeMap<_, _> = [
            ("approve-mode", "approve_mode"),
            ("verification-method", "verification_method"),
        ]
        .into_iter()
        .filter_map(|(source, target)| options.get(source).map(|v| (target, v)))
        .collect();
        if values.is_empty() {
            return Ok(None);
        }
        serde_json::from_value(serde_json::to_value(values)?).map(Some)
    }
}

#[derive(Debug, Serialize)]
pub struct Desired {
    pub schema_version: u8,
    pub revision: u64,
    pub settings: Settings,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Unsupported,
    UnverifiedCliMatch,
    ReadFailed,
    WriteFailed,
    Mismatch,
    Indeterminate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagementMode {
    ObserveOnly,
    Managed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Results {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub approve_mode: Option<Outcome>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub verification_method: Option<Outcome>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub schema_version: u8,
    pub agent_uuid: String,
    #[serde(deserialize_with = "nullable")]
    pub rustdesk_id: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub rustdesk_version: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub desired_revision: Option<u64>,
    pub observation_sequence: u64,
    #[serde(deserialize_with = "nullable")]
    pub applied_revision: Option<u64>,
    pub results: Results,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub management_mode: Option<ManagementMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "local_ips"
    )]
    pub local_ips: Option<Vec<std::net::IpAddr>>,
}
fn local_ips<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<std::net::IpAddr>>, D::Error> {
    let ips = Vec::<std::net::IpAddr>::deserialize(d)?;
    if ips.len() > 64 || ips.iter().enumerate().any(|(i, ip)| ips[..i].contains(ip)) {
        return Err(serde::de::Error::custom("invalid local IP list"));
    }
    Ok(Some(ips))
}
impl Report {
    pub fn valid_for(&self, settings: Option<&Settings>) -> bool {
        self.schema_version == 1
            && self.desired_revision.is_none_or(|revision| revision > 0)
            && self.desired_revision.is_some() == settings.is_some()
            && self.observation_sequence > 0
            && self.applied_revision.is_none()
            && self.results.approve_mode.is_some()
                == settings.is_some_and(|s| s.approve_mode.is_some())
            && self.results.verification_method.is_some()
                == settings.is_some_and(|s| s.verification_method.is_some())
            && self.rustdesk_id.as_ref().is_none_or(|s| s.len() <= 128)
            && self
                .rustdesk_version
                .as_ref()
                .is_none_or(|s| s.len() <= 128)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn local_ips_are_optional_bounded_and_validated() {
        let body = json!({"schema_version":1,"agent_uuid":"test","rustdesk_id":null,"rustdesk_version":null,"desired_revision":null,"observation_sequence":1,"applied_revision":null,"results":{}});
        assert!(serde_json::from_value::<Report>(body.clone())
            .unwrap()
            .valid_for(None));
        let mut valid = body.clone();
        valid["local_ips"] = json!(["192.168.1.20", "2001:db8::1"]);
        assert!(serde_json::from_value::<Report>(valid)
            .unwrap()
            .valid_for(None));
        for ips in [
            json!(null),
            json!(["bad"]),
            json!(["192.168.1.20", "192.168.1.20"]),
            json!(vec!["192.168.1.20"; 65]),
        ] {
            let mut invalid = body.clone();
            invalid["local_ips"] = ips;
            assert!(serde_json::from_value::<Report>(invalid).is_err());
        }
    }
    #[test]
    fn projects_only_allowlisted_values_and_rejects_invalid_selected_values() {
        let mut options =
            std::collections::HashMap::from([("proxy-password".into(), "not-forwarded".into())]);
        assert_eq!(Settings::from_options(&options).unwrap(), None);
        options.insert("approve-mode".into(), "click".into());
        assert_eq!(
            serde_json::to_value(Settings::from_options(&options).unwrap()).unwrap(),
            json!({"approve_mode":"click"})
        );
        options.insert("verification-method".into(), "invalid".into());
        assert!(Settings::from_options(&options).is_err());
        for raw in [
            r#"{"approve_mode":null}"#,
            r#"{"approve_mode":"click","approve_mode":"password"}"#,
            r#"{"unknown":"click"}"#,
        ] {
            assert!(serde_json::from_str::<Settings>(raw).is_err());
        }
    }
    #[test]
    fn reports_cannot_claim_verified_or_omit_required_nullable_fields() {
        let body = json!({"schema_version":1,"agent_uuid":"test","rustdesk_id":null,"rustdesk_version":null,"desired_revision":1,"observation_sequence":1,"applied_revision":null,"results":{"approve_mode":"unsupported"}});
        let settings = Settings {
            approve_mode: Some(Approval::Click),
            verification_method: None,
        };
        assert!(serde_json::from_value::<Report>(body.clone())
            .unwrap()
            .valid_for(Some(&settings)));
        for field in [
            "rustdesk_id",
            "rustdesk_version",
            "applied_revision",
            "desired_revision",
        ] {
            let mut missing = body.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<Report>(missing).is_err());
        }
        for value in [json!(null), json!("verified")] {
            let mut invalid = body.clone();
            invalid["results"]["approve_mode"] = value;
            assert!(serde_json::from_value::<Report>(invalid).is_err());
        }
        let mut applied = body;
        applied["applied_revision"] = json!(1);
        assert!(!serde_json::from_value::<Report>(applied)
            .unwrap()
            .valid_for(Some(&settings)));
    }
}
