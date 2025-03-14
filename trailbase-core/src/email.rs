use lettre::message::{header::ContentType, Body, Mailbox, Message};
use lettre::transport::smtp;
use lettre::{AsyncSendmailTransport, AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use minijinja::{context, Environment};
use std::sync::Arc;
use thiserror::Error;

use crate::auth::user::DbUser;
use crate::config::proto::{Config, EmailTemplate};
use crate::AppState;

#[derive(Debug, Error)]
pub enum EmailError {
  #[error("Email address error: {0}")]
  Address(#[from] lettre::address::AddressError),
  #[error("Missing error: {0}")]
  Missing(&'static str),
  #[error("Senda error: {0}")]
  Send(#[from] lettre::error::Error),
  #[error("SMTP error: {0}")]
  Smtp(#[from] lettre::transport::smtp::Error),
  #[error("Sendmail error: {0}")]
  Sendmail(#[from] lettre::transport::sendmail::Error),
  #[error("Template error: {0}")]
  Template(#[from] minijinja::Error),
}

pub struct Email {
  mailer: Arc<Mailer>,

  from: Mailbox,
  to: Mailbox,

  subject: String,
  body: String,
}

impl Email {
  pub fn new(
    state: &AppState,
    to: String,
    subject: String,
    body: String,
  ) -> Result<Self, EmailError> {
    return Ok(Self {
      mailer: state.mailer().clone(),
      from: get_sender(state)?,
      to: to.parse()?,
      subject,
      body,
    });
  }

  pub async fn send(&self) -> Result<(), EmailError> {
    let email = Message::builder()
      .to(self.to.clone())
      .from(self.from.clone())
      .subject(self.subject.clone())
      .header(ContentType::TEXT_HTML)
      .body(Body::new(self.body.clone()))?;

    match &*self.mailer {
      Mailer::Smtp(mailer) => {
        mailer.send(email).await?;
      }
      Mailer::Local(mailer) => {
        mailer.send(email).await?;
      }
    };

    return Ok(());
  }

  pub(crate) fn verification_email(
    state: &AppState,
    user: &DbUser,
    email_verification_code: &str,
  ) -> Result<Self, EmailError> {
    let (server_config, template) =
      state.access_config(|c| (c.server.clone(), c.email.user_verification_template.clone()));

    let Some(ref site_url) = server_config.site_url else {
      return Err(EmailError::Missing("config.site_url"));
    };

    let (subject_template, body_template) = match template {
      Some(EmailTemplate {
        subject: Some(subject),
        body: Some(body),
      }) => (subject, body),
      _ => {
        log::debug!("Falling back to default email verification email");
        let d = defaults::email_validation_email();
        (d.subject.unwrap(), d.body.unwrap())
      }
    };

    let verification_url = format!("{site_url}/verify_email/confirm/{email_verification_code}");

    let env = Environment::new();
    let subject = env
      .template_from_named_str("subject", &subject_template)?
      .render(context! {
        APP_NAME => server_config.application_name,
        EMAIL => user.email,
      })?;
    let body = env
      .template_from_named_str("body", &body_template)?
      .render(context! {
        APP_NAME => server_config.application_name,
        VERIFICATION_URL => verification_url,
        SITE_URL => server_config.site_url,
        CODE => email_verification_code,
        EMAIL => user.email,
      })?;

    return Email::new(state, user.email.clone(), subject, body);
  }

  pub(crate) fn change_email_address_email(
    state: &AppState,
    user: &DbUser,
    email_verification_code: &str,
  ) -> Result<Self, EmailError> {
    let (server_config, template) =
      state.access_config(|c| (c.server.clone(), c.email.change_email_template.clone()));

    let Some(ref site_url) = server_config.site_url else {
      return Err(EmailError::Missing("config.site_url"));
    };

    let (subject_template, body_template) = match template {
      Some(EmailTemplate {
        subject: Some(subject),
        body: Some(body),
      }) => (subject, body),
      _ => {
        log::debug!("Falling back to default change email template");
        let d = defaults::change_email_address_email();
        (d.subject.unwrap(), d.body.unwrap())
      }
    };

    let verification_url = format!("{site_url}/change_email/confirm/{email_verification_code}");

    let env = Environment::new();
    let subject = env
      .template_from_named_str("subject", &subject_template)?
      .render(context! {
        APP_NAME => server_config.application_name,
        EMAIL => user.email,
      })?;
    let body = env
      .template_from_named_str("body", &body_template)?
      .render(context! {
        APP_NAME => server_config.application_name,
        VERIFICATION_URL => verification_url,
        SITE_URL => server_config.site_url,
        CODE => email_verification_code,
        EMAIL => user.email,
      })?;

    return Email::new(state, user.email.clone(), subject, body);
  }

  pub(crate) fn password_reset_email(
    state: &AppState,
    user: &DbUser,
    password_reset_code: &str,
  ) -> Result<Self, EmailError> {
    let (server_config, template) =
      state.access_config(|c| (c.server.clone(), c.email.password_reset_template.clone()));

    let Some(ref site_url) = server_config.site_url else {
      return Err(EmailError::Missing("config.site_url"));
    };

    let (subject_template, body_template) = match template {
      Some(EmailTemplate {
        subject: Some(subject),
        body: Some(body),
      }) => (subject, body),
      _ => {
        log::debug!("Falling back to default reset password email");
        let d = defaults::password_reset_email();
        (d.subject.unwrap(), d.body.unwrap())
      }
    };

    let verification_url = format!("{site_url}/reset_password/update/{password_reset_code}");

    let env = Environment::new();
    let subject = env
      .template_from_named_str("subject", &subject_template)?
      .render(context! {
        APP_NAME => server_config.application_name,
        EMAIL => user.email,
      })?;
    let body = env
      .template_from_named_str("body", &body_template)?
      .render(context! {
        APP_NAME => server_config.application_name,
        VERIFICATION_URL => verification_url,
        SITE_URL => server_config.site_url,
        CODE => password_reset_code,
        EMAIL => user.email,
      })?;

    return Email::new(state, user.email.clone(), subject, body);
  }
}

fn get_sender(state: &AppState) -> Result<Mailbox, EmailError> {
  let (sender_address, sender_name) =
    state.access_config(|c| (c.email.sender_address.clone(), c.email.sender_name.clone()));
  // TODO: Have a better default sender, e.g. derive from SITE_URL.
  let address = sender_address.unwrap_or_else(|| "admin@localhost".to_string());

  if let Some(ref name) = sender_name {
    return Ok(format!("{} <{}>", name, address).parse::<Mailbox>()?);
  }
  return Ok(address.parse::<Mailbox>()?);
}

#[derive(Clone)]
pub(crate) enum Mailer {
  Smtp(Arc<dyn AsyncTransport<Ok = smtp::response::Response, Error = smtp::Error> + Send + Sync>),
  Local(Arc<AsyncSendmailTransport<Tokio1Executor>>),
}

impl Mailer {
  fn new_smtp(host: String, port: u16, user: String, pass: String) -> Result<Mailer, EmailError> {
    let mailer = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host)?
      .port(port)
      .credentials(smtp::authentication::Credentials::new(user, pass))
      .build();
    return Ok(Mailer::Smtp(Arc::new(mailer)));
  }

  fn new_local() -> Mailer {
    return Mailer::Local(Arc::new(AsyncSendmailTransport::<Tokio1Executor>::new()));
  }

  pub(crate) fn new_from_config(config: &Config) -> Mailer {
    let smtp_from_config = || -> Result<Mailer, EmailError> {
      let email = &config.email;
      let host = email
        .smtp_host
        .to_owned()
        .ok_or(EmailError::Missing("SMTP host"))?;
      let port = email
        .smtp_port
        .map(|port| port as u16)
        .ok_or(EmailError::Missing("SMTP port"))?;
      let user = email
        .smtp_username
        .to_owned()
        .ok_or(EmailError::Missing("SMTP username"))?;
      let pass = email
        .smtp_password
        .to_owned()
        .ok_or(EmailError::Missing("SMTP password"))?;

      Self::new_smtp(host, port, user, pass)
    };

    if let Ok(mailer) = smtp_from_config() {
      return mailer;
    }

    return Self::new_local();
  }
}

pub(crate) mod defaults {
  use crate::config::proto::EmailTemplate;
  use indoc::indoc;

  pub fn email_validation_email() -> EmailTemplate {
    const SUBJECT: &str = "Validate your Email Address for {{ APP_NAME }}";
    const BODY: &str = indoc! {r#"
        <html>
          <body>
            <h1>Welcome {{ EMAIL }}</h1>

            <p>
              Thanks for joining {{ APP_NAME }}.
            </p>

            <p>
              To be able to log in, first validate your email by clicking the link below.
            </p>

            <a class="btn" href="{{ VERIFICATION_URL }}">
              {{ VERIFICATION_URL }}
            </a>
          </body>
        </html>"#};

    return EmailTemplate {
      subject: Some(SUBJECT.to_string()),
      body: Some(BODY.to_string()),
    };
  }

  pub fn password_reset_email() -> EmailTemplate {
    const SUBJECT: &str = "Reset your Password for {{ APP_NAME }}";
    const BODY: &str = indoc! {r#"
        <html>
          <body>
            <h1>Password reset</h1>

            <p>
              Click the link below to reset your password.
            </p>

            <a class="btn" href="{{ VERIFICATION_URL }}">
              {{ VERIFICATION_URL }}
            </a>
          </body>
        </html>"#};

    return EmailTemplate {
      subject: Some(SUBJECT.to_string()),
      body: Some(BODY.to_string()),
    };
  }

  pub fn change_email_address_email() -> EmailTemplate {
    const SUBJECT: &str = "Change your Email Address for {{ APP_NAME }}";
    const BODY: &str = indoc! {r#"
        <html>
          <body>
            <h1>Change E-Mail Address</h1>

            <p>
              Click the link below to verify your new E-mail address:
            </p>

            <a class="btn" href="{{ VERIFICATION_URL }}">
              {{ VERIFICATION_URL }}
            </a>
          </body>
        </html>"#};

    return EmailTemplate {
      subject: Some(SUBJECT.to_string()),
      body: Some(BODY.to_string()),
    };
  }
}

#[cfg(test)]
pub mod testing {
  use std::sync::{Arc, Mutex};

  use lettre::address::Envelope;
  use lettre::transport::smtp::response::{Category, Code, Detail, Response, Severity};
  use lettre::AsyncTransport;

  #[derive(Clone)]
  pub struct TestAsyncSmtpTransport {
    response: Response,
    log: Arc<Mutex<Vec<(Envelope, String)>>>,
  }

  impl TestAsyncSmtpTransport {
    pub fn new() -> TestAsyncSmtpTransport {
      let code = Code::new(
        Severity::PositiveCompletion,
        Category::Information,
        Detail::Zero,
      );

      return TestAsyncSmtpTransport {
        response: Response::new(code, vec![]),
        log: Arc::new(Mutex::new(Vec::new())),
      };
    }

    pub fn get_logs(&self) -> Vec<(Envelope, String)> {
      return self.log.lock().unwrap().clone();
    }
  }

  #[async_trait::async_trait]
  impl AsyncTransport for TestAsyncSmtpTransport {
    type Ok = lettre::transport::smtp::response::Response;
    type Error = lettre::transport::smtp::Error;

    async fn send_raw(&self, envelope: &Envelope, email: &[u8]) -> Result<Self::Ok, Self::Error> {
      self
        .log
        .lock()
        .unwrap()
        .push((envelope.clone(), String::from_utf8_lossy(email).into()));

      return Ok(self.response.clone());
    }
  }
}
