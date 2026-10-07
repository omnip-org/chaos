//! Transactional email rendering shared by the worker and local preview.
//!
//! Rust prepares typed business data; MiniJinja owns presentation conditions
//! and repetition; MRML turns the rendered MJML into client-compatible HTML.

use std::path::{Path, PathBuf};

use minijinja::{AutoEscape, Environment, UndefinedBehavior};
use serde::Serialize;

use crate::contracts::{
    EmailBrandConfiguration, EmailOrderLineItem, FulfillmentEmailData, FulfillmentEmailStatus,
    OrderConfirmationEmailData,
};

const BASE_MJML: &str = include_str!("../templates/email/base.mjml");
const ORDER_STATUS_BUTTON_MJML: &str =
    include_str!("../templates/email/components/order-status-button.mjml");
const ORDER_CONFIRMED_SUBJECT: &str =
    include_str!("../templates/email/order-confirmed.subject.txt");
const ORDER_CONFIRMED_TEXT: &str = include_str!("../templates/email/order-confirmed.txt");
const ORDER_CONFIRMED_MJML: &str = include_str!("../templates/email/order-confirmed.mjml");
const FULFILLMENT_UPDATE_SUBJECT: &str =
    include_str!("../templates/email/fulfillment-update.subject.txt");
const FULFILLMENT_UPDATE_TEXT: &str = include_str!("../templates/email/fulfillment-update.txt");
const FULFILLMENT_UPDATE_MJML: &str = include_str!("../templates/email/fulfillment-update.mjml");

const TEMPLATE_NAMES: [&str; 8] = [
    "base.mjml",
    "components/order-status-button.mjml",
    "order-confirmed.subject.txt",
    "order-confirmed.txt",
    "order-confirmed.mjml",
    "fulfillment-update.subject.txt",
    "fulfillment-update.txt",
    "fulfillment-update.mjml",
];

#[derive(Debug, thiserror::Error)]
pub enum EmailTemplateError {
    #[error("failed to read email template {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("email template {template} is invalid: {message}")]
    Template { template: String, message: String },
    #[error("rendered MJML for {template} is invalid: {message}")]
    Mjml { template: String, message: String },
}

#[derive(Clone)]
pub struct EmailTemplateRenderer {
    environment: Environment<'static>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct RenderedEmailTemplate {
    pub subject: String,
    pub text: String,
    pub html: String,
}

impl EmailTemplateRenderer {
    /// Loads the production templates embedded in the binary and renders all
    /// representative branches before the worker starts accepting jobs.
    pub fn embedded() -> Result<Self, EmailTemplateError> {
        Self::from_sources([
            ("base.mjml", BASE_MJML.to_owned()),
            (
                "components/order-status-button.mjml",
                ORDER_STATUS_BUTTON_MJML.to_owned(),
            ),
            (
                "order-confirmed.subject.txt",
                ORDER_CONFIRMED_SUBJECT.to_owned(),
            ),
            ("order-confirmed.txt", ORDER_CONFIRMED_TEXT.to_owned()),
            ("order-confirmed.mjml", ORDER_CONFIRMED_MJML.to_owned()),
            (
                "fulfillment-update.subject.txt",
                FULFILLMENT_UPDATE_SUBJECT.to_owned(),
            ),
            ("fulfillment-update.txt", FULFILLMENT_UPDATE_TEXT.to_owned()),
            (
                "fulfillment-update.mjml",
                FULFILLMENT_UPDATE_MJML.to_owned(),
            ),
        ])
    }

    /// Loads templates from disk for the preview server. A new renderer should
    /// be created for each preview request so a browser refresh sees edits.
    pub fn from_directory(root: impl AsRef<Path>) -> Result<Self, EmailTemplateError> {
        let root = root.as_ref();
        let mut sources = Vec::with_capacity(TEMPLATE_NAMES.len());
        for name in TEMPLATE_NAMES {
            let path = root.join(name);
            let source =
                std::fs::read_to_string(&path).map_err(|source| EmailTemplateError::Read {
                    path: path.clone(),
                    source,
                })?;
            sources.push((name, source));
        }
        Self::from_sources(sources)
    }

    pub fn render_order_confirmation(
        &self,
        data: &OrderConfirmationEmailData,
    ) -> Result<RenderedEmailTemplate, EmailTemplateError> {
        let view = OrderConfirmationView::from(data);
        self.render(
            "order-confirmed.subject.txt",
            "order-confirmed.txt",
            "order-confirmed.mjml",
            &view,
        )
    }

    pub fn render_fulfillment_update(
        &self,
        data: &FulfillmentEmailData,
    ) -> Result<RenderedEmailTemplate, EmailTemplateError> {
        let view = FulfillmentUpdateView::from(data);
        self.render(
            "fulfillment-update.subject.txt",
            "fulfillment-update.txt",
            "fulfillment-update.mjml",
            &view,
        )
    }

    fn from_sources(
        sources: impl IntoIterator<Item = (&'static str, String)>,
    ) -> Result<Self, EmailTemplateError> {
        let mut environment = Environment::new();
        environment.set_undefined_behavior(UndefinedBehavior::Strict);
        environment.set_auto_escape_callback(|name| {
            if name.ends_with(".mjml") {
                AutoEscape::Html
            } else {
                AutoEscape::None
            }
        });
        for (name, source) in sources {
            environment
                .add_template_owned(name, source)
                .map_err(|error| EmailTemplateError::Template {
                    template: name.to_owned(),
                    message: error.to_string(),
                })?;
        }
        for name in TEMPLATE_NAMES {
            environment
                .get_template(name)
                .map_err(|error| EmailTemplateError::Template {
                    template: name.to_owned(),
                    message: error.to_string(),
                })?;
        }
        let renderer = Self { environment };
        renderer.validate_templates()?;
        Ok(renderer)
    }

    fn render<T: Serialize>(
        &self,
        subject_name: &str,
        text_name: &str,
        mjml_name: &str,
        view: &T,
    ) -> Result<RenderedEmailTemplate, EmailTemplateError> {
        let subject = self.render_template(subject_name, view)?.trim().to_owned();
        let text = self.render_template(text_name, view)?;
        let mjml = self.render_template(mjml_name, view)?;
        let html = render_mjml(mjml_name, &mjml)?;
        Ok(RenderedEmailTemplate {
            subject,
            text,
            html,
        })
    }

    fn render_template<T: Serialize>(
        &self,
        name: &str,
        view: &T,
    ) -> Result<String, EmailTemplateError> {
        self.environment
            .get_template(name)
            .and_then(|template| template.render(view))
            .map_err(|error| EmailTemplateError::Template {
                template: name.to_owned(),
                message: error.to_string(),
            })
    }

    fn validate_templates(&self) -> Result<(), EmailTemplateError> {
        let address = chaos_domain::sales::PostalAddress::new(
            "Preview Buyer",
            "1 Template Street",
            Some("Suite 2".into()),
            "Singapore",
            None,
            Some("018987".into()),
            "SG",
        )
        .map_err(|error| EmailTemplateError::Template {
            template: "built-in validation data".into(),
            message: error.to_string(),
        })?;
        let mut order = OrderConfirmationEmailData {
            recipient_name: Some("Preview Buyer".into()),
            order_number: "PREVIEW-1".into(),
            subtotal_amount_minor: 1_000,
            discount_amount_minor: 100,
            tax_amount_minor: 50,
            shipping_amount_minor: 100,
            total_amount_minor: 1_050,
            currency: "USD".into(),
            lookup_url: "https://shop.example.test/orders/details".into(),
            brand: EmailBrandConfiguration {
                logo_url: Some("https://cdn.example.test/logo.png".into()),
                ..EmailBrandConfiguration::defaults("Preview Store".into())
            },
            line_items: vec![EmailOrderLineItem {
                product_title: "Preview product".into(),
                variant_title: "Default".into(),
                sku: Some("PREVIEW-SKU".into()),
                quantity: 1,
                unit_price_amount_minor: 1_000,
                subtotal_amount_minor: 1_000,
                image_url: Some("https://cdn.example.test/product.png".into()),
            }],
            shipping_address: Some(address),
        };
        self.render_order_confirmation(&order)?;
        order.brand.logo_url = None;
        order.discount_amount_minor = 0;
        order.line_items.clear();
        order.shipping_address = None;
        order.recipient_name = None;
        self.render_order_confirmation(&order)?;

        let mut fulfillment = FulfillmentEmailData {
            recipient_name: Some("Preview Buyer".into()),
            order_number: "PREVIEW-1".into(),
            status: FulfillmentEmailStatus::Shipped,
            tracking_number: Some("TRACK-1".into()),
            tracking_url: Some("https://tracking.example.test/TRACK-1".into()),
            lookup_url: "https://shop.example.test/orders/details".into(),
            brand: EmailBrandConfiguration::defaults("Preview Store".into()),
        };
        self.render_fulfillment_update(&fulfillment)?;
        fulfillment.recipient_name = None;
        fulfillment.tracking_number = None;
        fulfillment.tracking_url = None;
        self.render_fulfillment_update(&fulfillment)?;
        fulfillment.status = FulfillmentEmailStatus::Delivered;
        self.render_fulfillment_update(&fulfillment)?;
        Ok(())
    }
}

fn render_mjml(template: &str, source: &str) -> Result<String, EmailTemplateError> {
    let parsed = mrml::parse(source).map_err(|error| EmailTemplateError::Mjml {
        template: template.to_owned(),
        message: error.to_string(),
    })?;
    if !parsed.warnings.is_empty() {
        return Err(EmailTemplateError::Mjml {
            template: template.to_owned(),
            message: format!("parser warnings: {:?}", parsed.warnings),
        });
    }
    parsed
        .element
        .render(&mrml::prelude::render::RenderOptions::default())
        .map_err(|error| EmailTemplateError::Mjml {
            template: template.to_owned(),
            message: error.to_string(),
        })
}

#[derive(Serialize)]
struct BrandView<'a> {
    name: &'a str,
    logo_url: Option<&'a str>,
    primary_color: &'a str,
    accent_color: &'a str,
    background_color: &'a str,
    surface_color: &'a str,
    text_color: &'a str,
    muted_text_color: &'a str,
}

impl<'a> From<&'a EmailBrandConfiguration> for BrandView<'a> {
    fn from(brand: &'a EmailBrandConfiguration) -> Self {
        Self {
            name: &brand.brand_name,
            logo_url: brand.logo_url.as_deref(),
            primary_color: &brand.primary_color,
            accent_color: &brand.accent_color,
            background_color: &brand.background_color,
            surface_color: &brand.surface_color,
            text_color: &brand.text_color,
            muted_text_color: &brand.muted_text_color,
        }
    }
}

#[derive(Serialize)]
struct OrderLineView<'a> {
    product_title: &'a str,
    variant_title: &'a str,
    sku: Option<&'a str>,
    quantity: i32,
    subtotal_amount: String,
    image_url: Option<&'a str>,
}

#[derive(Serialize)]
struct ShippingAddressView<'a> {
    full_name: &'a str,
    lines: Vec<String>,
}

#[derive(Serialize)]
struct OrderConfirmationView<'a> {
    brand: BrandView<'a>,
    recipient_name: Option<&'a str>,
    order_number: &'a str,
    subtotal_amount: String,
    discount_amount: Option<String>,
    tax_amount: String,
    shipping_amount: String,
    total_amount: String,
    currency: &'a str,
    lookup_url: &'a str,
    line_items: Vec<OrderLineView<'a>>,
    shipping_address: Option<ShippingAddressView<'a>>,
}

impl<'a> From<&'a OrderConfirmationEmailData> for OrderConfirmationView<'a> {
    fn from(data: &'a OrderConfirmationEmailData) -> Self {
        Self {
            brand: BrandView::from(&data.brand),
            recipient_name: data.recipient_name.as_deref(),
            order_number: &data.order_number,
            subtotal_amount: format_money(data.subtotal_amount_minor, &data.currency),
            discount_amount: (data.discount_amount_minor > 0)
                .then(|| format_money(data.discount_amount_minor, &data.currency)),
            tax_amount: format_money(data.tax_amount_minor, &data.currency),
            shipping_amount: format_money(data.shipping_amount_minor, &data.currency),
            total_amount: format_money(data.total_amount_minor, &data.currency),
            currency: &data.currency,
            lookup_url: &data.lookup_url,
            line_items: data
                .line_items
                .iter()
                .map(|item| OrderLineView {
                    product_title: &item.product_title,
                    variant_title: &item.variant_title,
                    sku: item.sku.as_deref(),
                    quantity: item.quantity,
                    subtotal_amount: format_money(item.subtotal_amount_minor, &data.currency),
                    image_url: item.image_url.as_deref(),
                })
                .collect(),
            shipping_address: data.shipping_address.as_ref().map(|address| {
                let mut lines = vec![address.address_line1().to_owned()];
                if let Some(line2) = address.address_line2() {
                    lines.push(line2.to_owned());
                }
                lines.push(address_locality_line(address));
                lines.push(address.country_code().to_owned());
                ShippingAddressView {
                    full_name: address.full_name(),
                    lines,
                }
            }),
        }
    }
}

#[derive(Serialize)]
struct FulfillmentUpdateView<'a> {
    brand: BrandView<'a>,
    recipient_name: Option<&'a str>,
    order_number: &'a str,
    status: &'static str,
    tracking_number: Option<&'a str>,
    tracking_url: Option<&'a str>,
    lookup_url: &'a str,
}

impl<'a> From<&'a FulfillmentEmailData> for FulfillmentUpdateView<'a> {
    fn from(data: &'a FulfillmentEmailData) -> Self {
        let (tracking_number, tracking_url) = match data.status {
            FulfillmentEmailStatus::Shipped => (
                data.tracking_number.as_deref(),
                data.tracking_url.as_deref(),
            ),
            FulfillmentEmailStatus::Delivered => (None, None),
        };
        Self {
            brand: BrandView::from(&data.brand),
            recipient_name: data.recipient_name.as_deref(),
            order_number: &data.order_number,
            status: data.status.as_str(),
            tracking_number,
            tracking_url,
            lookup_url: &data.lookup_url,
        }
    }
}

fn address_locality_line(address: &chaos_domain::sales::PostalAddress) -> String {
    match (address.administrative_area(), address.postal_code()) {
        (Some(area), Some(postal_code)) => {
            format!("{}, {} {}", address.locality(), area, postal_code)
        }
        (Some(area), None) => format!("{}, {}", address.locality(), area),
        (None, Some(postal_code)) => format!("{}, {}", address.locality(), postal_code),
        (None, None) => address.locality().to_owned(),
    }
}

fn format_money(amount_minor: i64, currency: &str) -> String {
    let exponent = currency_exponent(currency);
    let absolute = i128::from(amount_minor).abs();
    let scale = 10_i128.pow(exponent);
    let major = absolute / scale;
    let sign = if amount_minor < 0 { "-" } else { "" };
    if exponent == 0 {
        return format!("{sign}{major}");
    }
    let fraction = absolute % scale;
    format!(
        "{sign}{major}.{:0width$}",
        fraction,
        width = exponent as usize
    )
}

fn currency_exponent(currency: &str) -> u32 {
    match currency.to_ascii_uppercase().as_str() {
        "BIF" | "CLP" | "DJF" | "GNF" | "JPY" | "KMF" | "KRW" | "MGA" | "PYG" | "RWF" | "UGX"
        | "VND" | "VUV" | "XAF" | "XOF" | "XPF" => 0,
        "BHD" | "JOD" | "KWD" | "OMR" | "TND" => 3,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use chaos_domain::sales::PostalAddress;

    use crate::contracts::{
        EmailBrandConfiguration, EmailOrderLineItem, FulfillmentEmailData, FulfillmentEmailStatus,
        OrderConfirmationEmailData,
    };

    use super::EmailTemplateRenderer;

    #[test]
    fn renders_shipped_notice_with_escaped_tracking() {
        let rendered = EmailTemplateRenderer::embedded()
            .unwrap()
            .render_fulfillment_update(&FulfillmentEmailData {
                recipient_name: Some("A <Buyer>".into()),
                order_number: "ORD-<7>".into(),
                status: FulfillmentEmailStatus::Shipped,
                tracking_number: Some("1Z<99>".into()),
                tracking_url: Some("https://track.example/pkg?id=1&x=2".into()),
                lookup_url: "https://shop.example/orders/details?order_number=W-1&email=a&b".into(),
                brand: EmailBrandConfiguration::defaults("A <Store>".into()),
            })
            .unwrap();

        assert_eq!(rendered.subject, "Your order has shipped — ORD-<7>");
        assert!(rendered.text.contains("on its way to you."));
        assert!(rendered.text.contains("Tracking number: 1Z<99>"));
        assert!(
            rendered
                .text
                .contains("Track your shipment: https://track.example/pkg?id=1&x=2")
        );
        assert!(rendered.html.contains("ORD-&lt;7&gt;"));
        assert!(rendered.html.contains("Hi A &lt;Buyer&gt;"));
        assert!(rendered.html.contains("1Z&lt;99&gt;"));
        assert!(
            rendered
                .html
                .contains("https:&#x2f;&#x2f;track.example&#x2f;pkg?id=1&amp;x=2")
        );
        assert!(!rendered.html.contains("1Z<99>"));
        assert_no_template_syntax(&rendered.html);
    }

    #[test]
    fn renders_delivered_notice_without_tracking() {
        let rendered = EmailTemplateRenderer::embedded()
            .unwrap()
            .render_fulfillment_update(&FulfillmentEmailData {
                recipient_name: None,
                order_number: "ORD-8".into(),
                status: FulfillmentEmailStatus::Delivered,
                tracking_number: Some("SHOULD-NOT-APPEAR".into()),
                tracking_url: Some("https://track.example/x".into()),
                lookup_url: "https://shop.example/lookup".into(),
                brand: EmailBrandConfiguration::defaults("Example Store".into()),
            })
            .unwrap();

        assert_eq!(rendered.subject, "Your order was delivered — ORD-8");
        assert!(rendered.text.contains("has been delivered"));
        assert!(!rendered.text.contains("Tracking"));
        assert!(!rendered.text.contains("SHOULD-NOT-APPEAR"));
        assert!(!rendered.html.contains("SHOULD-NOT-APPEAR"));
        assert_no_template_syntax(&rendered.html);
    }

    #[test]
    fn renders_brand_and_order_snapshot_in_text_and_html() {
        let shipping_address = PostalAddress::new(
            "Buyer & Co.",
            "1 Market <Street>",
            Some("Suite 42".into()),
            "San Francisco",
            Some("CA".into()),
            Some("94105".into()),
            "US",
        )
        .unwrap();
        let rendered = EmailTemplateRenderer::embedded()
            .unwrap()
            .render_order_confirmation(&OrderConfirmationEmailData {
                recipient_name: Some("Buyer & Co.".into()),
                order_number: "ORD-<42>".into(),
                subtotal_amount_minor: 1300,
                discount_amount_minor: 100,
                tax_amount_minor: 50,
                shipping_amount_minor: 99,
                total_amount_minor: 1349,
                currency: "USD".into(),
                lookup_url: "https://shop.example/orders/details?order_number=W-1&email=a&b".into(),
                brand: EmailBrandConfiguration {
                    brand_name: "A <Store>".into(),
                    logo_url: Some("https://cdn.example/logo?a=1&b=2".into()),
                    ..EmailBrandConfiguration::defaults("Fallback".into())
                },
                line_items: vec![EmailOrderLineItem {
                    product_title: "T-shirt <classic>".into(),
                    variant_title: "Blue / M".into(),
                    sku: Some("TS-01".into()),
                    quantity: 2,
                    unit_price_amount_minor: 650,
                    subtotal_amount_minor: 1300,
                    image_url: Some("https://cdn.example/tshirt.png?a=1&b=2".into()),
                }],
                shipping_address: Some(shipping_address),
            })
            .unwrap();

        assert_eq!(rendered.subject, "Order confirmed — ORD-<42>");
        assert!(rendered.text.contains("T-shirt <classic> / Blue / M"));
        assert!(rendered.text.contains("Subtotal: 13.00 USD"));
        assert!(rendered.text.contains("Discount: -1.00 USD"));
        assert!(rendered.text.contains("Shipping: 0.99 USD"));
        assert!(rendered.text.contains("Tax: 0.50 USD"));
        assert!(rendered.text.contains("Total: 13.49 USD"));
        assert!(rendered.text.contains("Buyer & Co."));
        assert!(rendered.html.contains("A &lt;Store&gt;"));
        assert!(rendered.html.contains("T-shirt &lt;classic&gt;"));
        assert!(rendered.html.contains("Buyer &amp; Co."));
        assert!(rendered.html.contains("1 Market &lt;Street&gt;"));
        assert!(
            rendered
                .html
                .contains("https:&#x2f;&#x2f;cdn.example&#x2f;tshirt.png?a=1&amp;b=2")
        );
        assert!(
            rendered
                .html
                .contains("https:&#x2f;&#x2f;cdn.example&#x2f;logo?a=1&amp;b=2")
        );
        assert!(
            rendered
                .html
                .contains("https:&#x2f;&#x2f;shop.example&#x2f;orders&#x2f;details?order_number=W-1&amp;email=a&amp;b")
        );
        assert!(!rendered.html.contains("T-shirt <classic>"));
        assert_no_template_syntax(&rendered.html);
    }

    #[test]
    fn renders_optional_order_sections_and_currency_exponents() {
        let renderer = EmailTemplateRenderer::embedded().unwrap();
        let mut data = OrderConfirmationEmailData {
            recipient_name: None,
            order_number: "ORD-43".into(),
            subtotal_amount_minor: 1234,
            discount_amount_minor: 0,
            tax_amount_minor: 0,
            shipping_amount_minor: 0,
            total_amount_minor: 1234,
            currency: "JPY".into(),
            lookup_url: "https://shop.example/lookup".into(),
            brand: EmailBrandConfiguration::defaults("Example Store".into()),
            line_items: Vec::new(),
            shipping_address: None,
        };
        let rendered = renderer.render_order_confirmation(&data).unwrap();
        assert!(rendered.text.contains("No item details available."));
        assert!(rendered.text.contains("Total: 1234 JPY"));
        assert!(rendered.html.contains("No item details available."));
        assert!(!rendered.text.contains("Shipping address:"));
        assert!(!rendered.html.contains("Shipping address"));
        assert!(!rendered.text.contains("Discount:"));
        assert!(!rendered.html.contains(">Discount</"));

        data.currency = "KWD".into();
        let rendered = renderer.render_order_confirmation(&data).unwrap();
        assert!(rendered.text.contains("Total: 1.234 KWD"));
        assert_no_template_syntax(&rendered.html);
    }

    fn assert_no_template_syntax(value: &str) {
        assert!(!value.contains("{{"));
        assert!(!value.contains("{%"));
    }
}
