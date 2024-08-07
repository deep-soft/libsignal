//
// Copyright 2021 Signal Messenger, LLC.
// SPDX-License-Identifier: AGPL-3.0-only
//
use std::fmt;

use libsignal_net::chat::ChatServiceError;
use libsignal_net::svr3::Error as Svr3Error;
use signal_media::sanitize::mp4::{Error as Mp4Error, ParseError as Mp4ParseError};
use signal_media::sanitize::webp::{Error as WebpError, ParseError as WebpParseError};

use super::*;

const ERRORS_PROPERTY_NAME: &str = "Errors";
const ERROR_CLASS_NAME: &str = "LibSignalErrorBase";

#[allow(non_snake_case)]
fn node_registerErrors(mut cx: FunctionContext) -> JsResult<JsValue> {
    let errors_module = cx.argument::<JsObject>(0)?;
    cx.this::<JsObject>()?
        .set(&mut cx, ERRORS_PROPERTY_NAME, errors_module)?;
    Ok(cx.undefined().upcast())
}
node_register!(registerErrors);

fn new_js_error<'a>(
    cx: &mut impl Context<'a>,
    module: Handle<'a, JsObject>,
    name: Option<&str>,
    message: &str,
    operation: &str,
    extra_props: Option<Handle<'a, JsObject>>,
) -> Option<Handle<'a, JsObject>> {
    let result = cx.try_catch(|cx| {
        let errors_module: Handle<JsObject> = module.get(cx, ERRORS_PROPERTY_NAME)?;
        let error_class: Handle<JsFunction> = errors_module.get(cx, ERROR_CLASS_NAME)?;
        let name_arg = match name {
            Some(name) => cx.string(name).upcast(),
            None => cx.undefined().upcast(),
        };
        let extra_props_arg = match extra_props {
            Some(props) => props.upcast(),
            None => cx.undefined().upcast(),
        };

        let args: &[Handle<JsValue>] = &[
            cx.string(message).upcast(),
            name_arg,
            cx.string(operation).upcast(),
            extra_props_arg,
        ];
        error_class.construct(cx, args)
    });
    match result {
        Ok(error_instance) => Some(error_instance),
        Err(failure) => {
            log::warn!(
                "could not construct {}: {}",
                name.unwrap_or("LibSignalError"),
                failure
                    .to_string(cx)
                    .map(|s| s.value(cx))
                    .unwrap_or_else(|_| "(could not print error)".to_owned())
            );
            None
        }
    }
}

/// [`std::error::Error`] implementer that wraps a thrown value.
#[derive(Debug)]
pub(crate) enum ThrownException {
    Error(Root<JsError>),
    String(String),
}

impl ThrownException {
    pub(crate) fn from_value<'a>(cx: &mut FunctionContext<'a>, error: Handle<'a, JsValue>) -> Self {
        if let Ok(e) = error.downcast::<JsError, _>(cx) {
            ThrownException::Error(e.root(cx))
        } else if let Ok(e) = error.downcast::<JsString, _>(cx) {
            ThrownException::String(e.value(cx))
        } else {
            ThrownException::String(
                error
                    .to_string(cx)
                    .expect("can convert to string")
                    .value(cx),
            )
        }
    }
}

impl Default for ThrownException {
    fn default() -> Self {
        Self::String(String::default())
    }
}

impl From<&str> for ThrownException {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl std::fmt::Display for ThrownException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error(r) => write!(f, "{:?}", r),
            Self::String(s) => write!(f, "{}", s),
        }
    }
}

impl std::error::Error for ThrownException {}

pub trait SignalNodeError: Sized + fmt::Display {
    fn throw<'a>(
        self,
        cx: &mut impl Context<'a>,
        module: Handle<'a, JsObject>,
        operation_name: &str,
    ) -> JsResult<'a, JsValue> {
        let message = self.to_string();
        match new_js_error(cx, module, None, &message, operation_name, None) {
            Some(error) => cx.throw(error),
            None => {
                // Make sure we still throw something.
                cx.throw_error(&message)
            }
        }
    }
}

const INVALID_MEDIA_INPUT: &str = "InvalidMediaInput";
const IO_ERROR: &str = "IoError";
const RATE_LIMITED_ERROR: &str = "RateLimitedError";
const SVR3_DATA_MISSING: &str = "SvrDataMissing";
const SVR3_REQUEST_FAILED: &str = "SvrRequestFailed";
const SVR3_RESTORE_FAILED: &str = "SvrRestoreFailed";
const UNSUPPORTED_MEDIA_INPUT: &str = "UnsupportedMediaInput";

impl SignalNodeError for neon::result::Throw {
    fn throw<'a>(
        self,
        _cx: &mut impl Context<'a>,
        _module: Handle<'a, JsObject>,
        _operation_name: &str,
    ) -> JsResult<'a, JsValue> {
        Err(self)
    }
}

impl SignalNodeError for SignalProtocolError {
    fn throw<'a>(
        self,
        cx: &mut impl Context<'a>,
        module: Handle<'a, JsObject>,
        operation_name: &str,
    ) -> JsResult<'a, JsValue> {
        // Check for some dedicated error types first.
        let custom_error = match &self {
            SignalProtocolError::DuplicatedMessage(..) => new_js_error(
                cx,
                module,
                Some("DuplicatedMessage"),
                &self.to_string(),
                operation_name,
                None,
            ),
            SignalProtocolError::SealedSenderSelfSend => new_js_error(
                cx,
                module,
                Some("SealedSenderSelfSend"),
                &self.to_string(),
                operation_name,
                None,
            ),
            SignalProtocolError::UntrustedIdentity(addr) => {
                let props = cx.empty_object();
                let addr_string = cx.string(addr.name());
                props.set(cx, "_addr", addr_string)?;
                new_js_error(
                    cx,
                    module,
                    Some("UntrustedIdentity"),
                    &self.to_string(),
                    operation_name,
                    Some(props),
                )
            }
            SignalProtocolError::InvalidRegistrationId(addr, _value) => {
                let props = cx.empty_object();
                let addr = addr.clone().convert_into(cx)?;
                props.set(cx, "_addr", addr)?;
                new_js_error(
                    cx,
                    module,
                    Some("InvalidRegistrationId"),
                    &self.to_string(),
                    operation_name,
                    Some(props),
                )
            }
            SignalProtocolError::InvalidSessionStructure(..) => new_js_error(
                cx,
                module,
                Some("InvalidSession"),
                &self.to_string(),
                operation_name,
                None,
            ),
            SignalProtocolError::InvalidSenderKeySession { distribution_id } => {
                let props = cx.empty_object();
                let distribution_id_str =
                    cx.string(format!("{:x}", distribution_id.as_hyphenated()));
                props.set(cx, "distribution_id", distribution_id_str)?;
                new_js_error(
                    cx,
                    module,
                    Some("InvalidSenderKeySession"),
                    &self.to_string(),
                    operation_name,
                    Some(props),
                )
            }
            _ => new_js_error(cx, module, None, &self.to_string(), operation_name, None),
        };

        match custom_error {
            Some(error) => cx.throw(error),
            None => {
                // Make sure we still throw something.
                cx.throw_error(self.to_string())
            }
        }
    }
}

impl SignalNodeError for device_transfer::Error {}

impl SignalNodeError for attest::hsm_enclave::Error {}

impl SignalNodeError for attest::enclave::Error {}

impl SignalNodeError for signal_crypto::Error {}

impl SignalNodeError for zkgroup::ZkGroupVerificationFailure {}

impl SignalNodeError for zkgroup::ZkGroupDeserializationFailure {}

impl SignalNodeError for usernames::UsernameError {
    fn throw<'a>(
        self,
        cx: &mut impl Context<'a>,
        module: Handle<'a, JsObject>,
        operation_name: &str,
    ) -> JsResult<'a, JsValue> {
        let name = match &self {
            Self::BadNicknameCharacter => "BadNicknameCharacter",
            Self::NicknameTooShort => "NicknameTooShort",
            Self::NicknameTooLong => "NicknameTooLong",
            Self::NicknameCannotBeEmpty => "NicknameCannotBeEmpty",
            Self::NicknameCannotStartWithDigit => "CannotStartWithDigit",
            Self::MissingSeparator => "MissingSeparator",
            Self::DiscriminatorCannotBeEmpty => "DiscriminatorCannotBeEmpty",
            Self::DiscriminatorCannotBeZero => "DiscriminatorCannotBeZero",
            Self::DiscriminatorCannotBeSingleDigit => "DiscriminatorCannotBeSingleDigit",
            Self::DiscriminatorCannotHaveLeadingZeros => "DiscriminatorCannotHaveLeadingZeros",
            Self::BadDiscriminatorCharacter => "BadDiscriminatorCharacter",
            Self::DiscriminatorTooLarge => "DiscriminatorTooLarge",
        };
        let message = self.to_string();
        match new_js_error(cx, module, Some(name), &message, operation_name, None) {
            Some(error) => cx.throw(error),
            None => {
                // Make sure we still throw something.
                cx.throw_error(message)
            }
        }
    }
}

impl SignalNodeError for usernames::ProofVerificationFailure {}

impl SignalNodeError for usernames::UsernameLinkError {
    fn throw<'a>(
        self,
        cx: &mut impl Context<'a>,
        module: Handle<'a, JsObject>,
        operation_name: &str,
    ) -> JsResult<'a, JsValue> {
        let name = match &self {
            Self::InputDataTooLong => Some("InputDataTooLong"),
            Self::InvalidEntropyDataLength => Some("InvalidEntropyDataLength"),
            Self::UsernameLinkDataTooShort
            | Self::HmacMismatch
            | Self::BadCiphertext
            | Self::InvalidDecryptedDataStructure => Some("InvalidUsernameLinkEncryptedData"),
        };
        let message = self.to_string();
        match new_js_error(cx, module, name, &message, operation_name, None) {
            Some(error) => cx.throw(error),
            None => {
                // Make sure we still throw something.
                cx.throw_error(message)
            }
        }
    }
}

impl SignalNodeError for Mp4Error {
    fn throw<'a>(
        self,
        cx: &mut impl Context<'a>,
        module: Handle<'a, JsObject>,
        operation_name: &str,
    ) -> JsResult<'a, JsValue> {
        let name = match &self {
            Mp4Error::Io(_) => IO_ERROR,
            Mp4Error::Parse(err) => match err.kind {
                Mp4ParseError::InvalidBoxLayout
                | Mp4ParseError::InvalidInput
                | Mp4ParseError::MissingRequiredBox(_)
                | Mp4ParseError::TruncatedBox => INVALID_MEDIA_INPUT,
                Mp4ParseError::UnsupportedBox(_)
                | Mp4ParseError::UnsupportedBoxLayout
                | Mp4ParseError::UnsupportedFormat(_) => UNSUPPORTED_MEDIA_INPUT,
            },
        };
        let message = self.to_string();
        match new_js_error(cx, module, Some(name), &message, operation_name, None) {
            Some(error) => cx.throw(error),
            None => {
                // Make sure we still throw something.
                cx.throw_error(&message)
            }
        }
    }
}

impl SignalNodeError for WebpError {
    fn throw<'a>(
        self,
        cx: &mut impl Context<'a>,
        module: Handle<'a, JsObject>,
        operation_name: &str,
    ) -> JsResult<'a, JsValue> {
        let name = match &self {
            WebpError::Io(_) => IO_ERROR,
            WebpError::Parse(err) => match err.kind {
                WebpParseError::InvalidChunkLayout
                | WebpParseError::InvalidInput
                | WebpParseError::InvalidVp8lPrefixCode
                | WebpParseError::MissingRequiredChunk(_)
                | WebpParseError::TruncatedChunk => INVALID_MEDIA_INPUT,
                WebpParseError::UnsupportedChunk(_) | WebpParseError::UnsupportedVp8lVersion(_) => {
                    UNSUPPORTED_MEDIA_INPUT
                }
            },
        };
        let message = self.to_string();
        match new_js_error(cx, module, Some(name), &message, operation_name, None) {
            Some(error) => cx.throw(error),
            None => {
                // Make sure we still throw something.
                cx.throw_error(&message)
            }
        }
    }
}

impl SignalNodeError for std::io::Error {
    fn throw<'a>(
        mut self,
        cx: &mut impl Context<'a>,
        _module: Handle<'a, JsObject>,
        _operation_name: &str,
    ) -> JsResult<'a, JsValue> {
        let exception = (self.kind() == std::io::ErrorKind::Other)
            .then(|| {
                self.get_mut()
                    .and_then(|e| e.downcast_mut::<ThrownException>())
            })
            .flatten()
            .map(std::mem::take);

        match exception {
            Some(ThrownException::Error(e)) => {
                let inner = e.into_inner(cx);
                cx.throw(inner)
            }
            Some(ThrownException::String(s)) => cx.throw_error(s),
            None => cx.throw_error(self.to_string()),
        }
    }
}

impl SignalNodeError for libsignal_net::chat::ChatServiceError {
    fn throw<'a>(
        self,
        cx: &mut impl Context<'a>,
        module: Handle<'a, JsObject>,
        operation_name: &str,
    ) -> JsResult<'a, JsValue> {
        let name = match self {
            ChatServiceError::ServiceInactive => Some("ChatServiceInactive"),
            ChatServiceError::AppExpired => Some("AppExpired"),
            ChatServiceError::DeviceDeregistered => Some("DeviceDelinked"),
            // TODO: Distinguish retryable errors from proper failures?
            _ => Some(IO_ERROR),
        };
        let message = self.to_string();
        match new_js_error(cx, module, name, &message, operation_name, None) {
            Some(error) => cx.throw(error),
            None => {
                // Make sure we still throw something.
                cx.throw_error(message)
            }
        }
    }
}

impl SignalNodeError for http::uri::InvalidUri {
    fn throw<'a>(
        self,
        cx: &mut impl Context<'a>,
        module: Handle<'a, JsObject>,
        operation_name: &str,
    ) -> JsResult<'a, JsValue> {
        let name = Some("InvalidUri");
        let message = self.to_string();
        match new_js_error(cx, module, name, &message, operation_name, None) {
            Some(error) => cx.throw(error),
            None => {
                // Make sure we still throw something.
                cx.throw_error(message)
            }
        }
    }
}

impl SignalNodeError for libsignal_net::cdsi::LookupError {
    fn throw<'a>(
        self,
        cx: &mut impl Context<'a>,
        module: Handle<'a, JsObject>,
        operation_name: &str,
    ) -> JsResult<'a, JsValue> {
        let (name, extra_props) = match self {
            Self::RateLimited {
                retry_after_seconds,
            } => (
                Some(RATE_LIMITED_ERROR),
                Some({
                    let props = cx.empty_object();
                    let retry_after = retry_after_seconds.convert_into(cx)?;
                    props.set(cx, "retryAfterSecs", retry_after)?;
                    props
                }),
            ),
            Self::AttestationError(e) => return e.throw(cx, module, operation_name),
            Self::InvalidArgument { server_reason: _ } => (None, None),
            Self::InvalidToken => (Some("CdsiInvalidToken"), None),
            Self::ConnectionTimedOut
            | Self::ConnectTransport(_)
            | Self::WebSocket(_)
            | Self::Protocol
            | Self::InvalidResponse
            | Self::ParseError
            | Self::Server { reason: _ } => (Some(IO_ERROR), None),
        };
        let message = self.to_string();
        new_js_error(cx, module, name, &message, operation_name, extra_props)
            .map(|e| cx.throw(e))
            // Make sure we still throw something.
            .unwrap_or_else(|| cx.throw_error(&message))
    }
}

impl SignalNodeError for libsignal_net::svr3::Error {
    fn throw<'a>(
        self,
        cx: &mut impl Context<'a>,
        module: Handle<'a, JsObject>,
        operation_name: &str,
    ) -> JsResult<'a, JsValue> {
        let (name, props) = match self {
            Svr3Error::Service(_) | Svr3Error::ConnectionTimedOut | Svr3Error::Connect(_) => {
                (Some(IO_ERROR), None)
            }
            Svr3Error::AttestationError(inner) => {
                return inner.throw(cx, module, operation_name);
            }
            Svr3Error::RequestFailed(_) => (Some(SVR3_REQUEST_FAILED), None),
            Svr3Error::RestoreFailed(tries_remaining) => (
                Some(SVR3_RESTORE_FAILED),
                Some({
                    let props = cx.empty_object();
                    let tries_remaining = tries_remaining.convert_into(cx)?;
                    props.set(cx, "triesRemaining", tries_remaining)?;
                    props
                }),
            ),
            Svr3Error::DataMissing => (Some(SVR3_DATA_MISSING), None),
            Svr3Error::Protocol(_) => (None, None),
        };

        let message = self.to_string();
        match new_js_error(cx, module, name, &message, operation_name, props) {
            Some(error) => cx.throw(error),
            None => {
                // Make sure we still throw something.
                cx.throw_error(message)
            }
        }
    }
}

impl SignalNodeError for CancellationError {
    fn throw<'a>(
        self,
        cx: &mut impl Context<'a>,
        module: Handle<'a, JsObject>,
        operation_name: &str,
    ) -> JsResult<'a, JsValue> {
        let message = self.to_string();
        match new_js_error(
            cx,
            module,
            Some("Cancelled"),
            &message,
            operation_name,
            None,
        ) {
            Some(error) => cx.throw(error),
            None => {
                // Make sure we still throw something.
                cx.throw_error(&message)
            }
        }
    }
}

impl SignalNodeError for libsignal_message_backup::ReadError {
    fn throw<'a>(
        self,
        cx: &mut impl Context<'a>,
        module: Handle<'a, JsObject>,
        operation_name: &str,
    ) -> JsResult<'a, JsValue> {
        let libsignal_message_backup::ReadError {
            error,
            found_unknown_fields,
        } = self;
        let message = error.to_string();
        let props = cx.empty_object();
        let unknown_field_messages = found_unknown_fields.convert_into(cx)?;
        props.set(cx, "unknownFields", unknown_field_messages)?;
        match new_js_error(
            cx,
            module,
            Some("BackupValidation"),
            &message,
            operation_name,
            Some(props),
        ) {
            Some(error) => cx.throw(error),
            None => {
                // Make sure we still throw something.
                cx.throw_error(&message)
            }
        }
    }
}

/// Represents an error returned by a callback.
#[derive(Debug)]
struct CallbackError {
    message: String,
}

impl CallbackError {
    fn new(message: String) -> CallbackError {
        Self { message }
    }
}

impl fmt::Display for CallbackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "callback error {}", self.message)
    }
}

impl std::error::Error for CallbackError {}

/// Converts a JavaScript error message to a [`SignalProtocolError::ApplicationCallbackError`].
pub fn js_error_to_rust(func: &'static str, err: String) -> SignalProtocolError {
    SignalProtocolError::ApplicationCallbackError(func, Box::new(CallbackError::new(err)))
}
