use crate::contracts::{EmailBrandConfiguration, EmailOrderLineItem};
use chaos_domain::sales::PostalAddress;

const ORDER_CONFIRMED_SUBJECT: &str =
    include_str!("../templates/email/order-confirmed.subject.txt");
const ORDER_CONFIRMED_TEXT: &str = include_str!("../templates/email/order-confirmed.txt");
const ORDER_CONFIRMED_HTML: &str = include_str!("../templates/email/order-confirmed.html");

/// The template structure is owned by the platform. Store configuration only
/// supplies branding tokens; order data and repeated line-item fragments are
/// always assembled by the server from the order snapshot.
#[derive(Clone)]
pub(crate) struct EmailTemplateContent {
    pub subject_template: String,
    pub text_template: String,
    pub html_template: String,
}

pub(crate) fn default_order_confirmation_template() -> EmailTemplateContent {
    EmailTemplateContent {
        subject_template: ORDER_CONFIRMED_SUBJECT.trim_end().to_owned(),
        text_template: ORDER_CONFIRMED_TEXT.to_owned(),
        html_template: ORDER_CONFIRMED_HTML.to_owned(),
    }
}

pub(crate) struct OrderConfirmationTemplateData<'a> {
    pub order_number: &'a str,
    pub subtotal_amount_minor: i64,
    pub discount_amount_minor: i64,
    pub tax_amount_minor: i64,
    pub shipping_amount_minor: i64,
    pub total_amount_minor: i64,
    pub currency: &'a str,
    pub lookup_url: &'a str,
    pub brand: &'a EmailBrandConfiguration,
    pub line_items: &'a [EmailOrderLineItem],
    pub shipping_address: Option<&'a PostalAddress>,
}

pub(crate) struct RenderedEmailTemplate {
    pub subject: String,
    pub text: String,
    pub html: String,
}

pub(crate) fn render_order_confirmation(
    template: &EmailTemplateContent,
    data: &OrderConfirmationTemplateData<'_>,
) -> RenderedEmailTemplate {
    let order_number = data.order_number.to_owned();
    let currency = data.currency.to_owned();
    let subtotal_amount = format_money(data.subtotal_amount_minor, &currency);
    let discount_amount = format_money(data.discount_amount_minor, &currency);
    let tax_amount = format_money(data.tax_amount_minor, &currency);
    let shipping_amount = format_money(data.shipping_amount_minor, &currency);
    let total_amount = format_money(data.total_amount_minor, &currency);
    let lookup_url = data.lookup_url.to_owned();
    let line_items_text = render_line_items_text(data.line_items, &currency);
    let shipping_address_text = render_shipping_address_text(data.shipping_address);
    let discount_text =
        render_discount_text(data.discount_amount_minor, &discount_amount, &currency);
    let support_text = render_support_text(data.brand);

    let html_order_number = escape_html(&order_number);
    let html_subtotal_amount = escape_html(&subtotal_amount);
    let html_total_amount = escape_html(&total_amount);
    let html_currency = escape_html(&currency);
    let html_shipping_amount = escape_html(&shipping_amount);
    let html_tax_amount = escape_html(&tax_amount);
    let html_lookup_url = escape_html(&lookup_url);
    let html_brand_name = escape_html(&data.brand.brand_name);
    let html_primary_color = escape_html(&data.brand.primary_color);
    let html_accent_color = escape_html(&data.brand.accent_color);
    let html_background_color = escape_html(&data.brand.background_color);
    let html_surface_color = escape_html(&data.brand.surface_color);
    let html_text_color = escape_html(&data.brand.text_color);
    let html_muted_text_color = escape_html(&data.brand.muted_text_color);
    let brand_header_html = render_brand_header_html(data.brand);
    let line_items_html = render_line_items_html(data.line_items, &currency, data.brand);
    let shipping_address_html = render_shipping_address_html(data.shipping_address, data.brand);
    let discount_row_html = render_discount_row_html(
        data.discount_amount_minor,
        &discount_amount,
        &currency,
        data.brand,
    );
    let support_html = render_support_html(data.brand);

    let subject = render_template(
        &template.subject_template,
        &[
            ("brand_name", data.brand.brand_name.as_str()),
            ("order_number", order_number.as_str()),
            ("total_amount", total_amount.as_str()),
            ("currency", currency.as_str()),
            ("lookup_url", lookup_url.as_str()),
        ],
    )
    .trim()
    .to_owned();
    let text = render_template(
        &template.text_template,
        &[
            ("brand_name", data.brand.brand_name.as_str()),
            ("order_number", order_number.as_str()),
            ("subtotal_amount", subtotal_amount.as_str()),
            ("discount_text", discount_text.as_str()),
            ("shipping_amount", shipping_amount.as_str()),
            ("tax_amount", tax_amount.as_str()),
            ("total_amount", total_amount.as_str()),
            ("currency", currency.as_str()),
            ("lookup_url", lookup_url.as_str()),
            ("line_items_text", line_items_text.as_str()),
            ("shipping_address_text", shipping_address_text.as_str()),
            ("support_text", support_text.as_str()),
        ],
    );
    let html = render_template(
        &template.html_template,
        &[
            ("brand_name", html_brand_name.as_str()),
            ("brand_header_html", brand_header_html.as_str()),
            ("primary_color", html_primary_color.as_str()),
            ("accent_color", html_accent_color.as_str()),
            ("background_color", html_background_color.as_str()),
            ("surface_color", html_surface_color.as_str()),
            ("text_color", html_text_color.as_str()),
            ("muted_text_color", html_muted_text_color.as_str()),
            ("order_number", html_order_number.as_str()),
            ("subtotal_amount", html_subtotal_amount.as_str()),
            ("discount_row_html", discount_row_html.as_str()),
            ("shipping_amount", html_shipping_amount.as_str()),
            ("tax_amount", html_tax_amount.as_str()),
            ("total_amount", html_total_amount.as_str()),
            ("currency", html_currency.as_str()),
            ("lookup_url", html_lookup_url.as_str()),
            ("line_items_html", line_items_html.as_str()),
            ("shipping_address_html", shipping_address_html.as_str()),
            ("support_html", support_html.as_str()),
        ],
    );

    RenderedEmailTemplate {
        subject,
        text,
        html,
    }
}

fn render_template(template: &str, values: &[(&str, &str)]) -> String {
    let mut rendered = String::with_capacity(template.len());
    let mut cursor = 0;
    while let Some(start_offset) = template[cursor..].find("{{") {
        let start = cursor + start_offset;
        rendered.push_str(&template[cursor..start]);
        let key_start = start + 2;
        let Some(end_offset) = template[key_start..].find("}}") else {
            rendered.push_str(&template[start..]);
            return rendered;
        };
        let end = key_start + end_offset;
        let key = &template[key_start..end];
        if let Some((_, value)) = values.iter().find(|(name, _)| *name == key) {
            rendered.push_str(value);
        } else {
            rendered.push_str(&template[start..end + 2]);
        }
        cursor = end + 2;
    }
    rendered.push_str(&template[cursor..]);
    rendered
}

fn render_brand_header_html(brand: &EmailBrandConfiguration) -> String {
    let brand_name = escape_html(&brand.brand_name);
    match brand.logo_url.as_deref() {
        Some(logo_url) => format!(
            "<img src=\"{}\" alt=\"{}\" width=\"48\" height=\"48\" style=\"display:inline-block;vertical-align:middle;border:0;border-radius:12px;object-fit:contain;\" /><span style=\"display:inline-block;margin-left:12px;vertical-align:middle;line-height:48px;\">{}</span>",
            escape_html(logo_url),
            brand_name,
            brand_name,
        ),
        None => format!(
            "<span style=\"display:inline-block;line-height:48px;\">{}</span>",
            brand_name
        ),
    }
}

fn render_line_items_html(
    items: &[EmailOrderLineItem],
    currency: &str,
    brand: &EmailBrandConfiguration,
) -> String {
    let border_color = escape_html(&brand.accent_color);
    let muted_text_color = escape_html(&brand.muted_text_color);
    let text_color = escape_html(&brand.text_color);
    let mut rendered = format!(
        "<table role=\"presentation\" width=\"100%\" cellspacing=\"0\" cellpadding=\"0\" style=\"border-collapse:collapse;\"><thead><tr><th align=\"left\" style=\"padding:0 0 10px;border-bottom:1px solid {border_color};font-size:12px;color:{muted_text_color};font-weight:600;\">Item</th><th align=\"center\" style=\"padding:0 8px 10px;border-bottom:1px solid {border_color};font-size:12px;color:{muted_text_color};font-weight:600;\">Qty</th><th align=\"right\" style=\"padding:0 0 10px;border-bottom:1px solid {border_color};font-size:12px;color:{muted_text_color};font-weight:600;\">Amount</th></tr></thead><tbody>"
    );
    if items.is_empty() {
        rendered.push_str(&format!(
            "<tr><td colspan=\"3\" style=\"padding:16px 0;color:{muted_text_color};font-size:14px;\">No item details available.</td></tr>"
        ));
    } else {
        for item in items {
            let product_title = escape_html(&item.product_title);
            let variant_title = escape_html(&item.variant_title);
            let subtotal = escape_html(&format_money(item.subtotal_amount_minor, currency));
            let sku = item
                .sku
                .as_deref()
                .map(|sku| {
                    format!(
                        "<br /><span style=\"color:{muted_text_color};font-size:12px;\">SKU {}</span>",
                        escape_html(sku),
                    )
                })
                .unwrap_or_default();
            rendered.push_str(&format!(
                "<tr><td style=\"padding:14px 0;border-bottom:1px solid {border_color};font-size:14px;color:{text_color};\"><strong>{}</strong><br /><span style=\"color:{muted_text_color};font-size:12px;\">{}{}</span></td><td align=\"center\" style=\"padding:14px 8px;border-bottom:1px solid {border_color};font-size:14px;color:{text_color};\">{}</td><td align=\"right\" style=\"padding:14px 0;border-bottom:1px solid {border_color};font-size:14px;color:{text_color};white-space:nowrap;\">{} {}</td></tr>",
                product_title,
                variant_title,
                sku,
                item.quantity,
                subtotal,
                escape_html(currency),
            ));
        }
    }
    rendered.push_str("</tbody></table>");
    rendered
}

fn render_line_items_text(items: &[EmailOrderLineItem], currency: &str) -> String {
    if items.is_empty() {
        return "- No item details available.".into();
    }
    items
        .iter()
        .map(|item| {
            let sku = item
                .sku
                .as_deref()
                .map(|sku| format!(", SKU {sku}"))
                .unwrap_or_default();
            format!(
                "- {} / {}{} × {} — {} {}",
                item.product_title,
                item.variant_title,
                sku,
                item.quantity,
                format_money(item.subtotal_amount_minor, currency),
                currency,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_shipping_address_text(address: Option<&PostalAddress>) -> String {
    let Some(address) = address else {
        return String::new();
    };
    let mut lines = vec![
        address.full_name().to_owned(),
        address.address_line1().to_owned(),
    ];
    if let Some(line2) = address.address_line2() {
        lines.push(line2.to_owned());
    }
    lines.push(address_locality_line(address));
    lines.push(address.country_code().to_owned());
    format!("Shipping address:\n{}", lines.join("\n"))
}

fn render_discount_text(amount_minor: i64, amount: &str, currency: &str) -> String {
    if amount_minor > 0 {
        format!("Discount: -{amount} {currency}")
    } else {
        String::new()
    }
}

fn render_discount_row_html(
    amount_minor: i64,
    amount: &str,
    currency: &str,
    brand: &EmailBrandConfiguration,
) -> String {
    if amount_minor <= 0 {
        return String::new();
    }
    let muted_text_color = escape_html(&brand.muted_text_color);
    let text_color = escape_html(&brand.text_color);
    let amount = escape_html(&format!("-{amount}"));
    let currency = escape_html(currency);
    format!(
        "<tr><td style=\"padding:8px 16px;color:{muted_text_color}\">Discount</td><td align=\"right\" style=\"padding:8px 16px;color:{text_color}\">{amount} {currency}</td></tr>"
    )
}

fn render_shipping_address_html(
    address: Option<&PostalAddress>,
    brand: &EmailBrandConfiguration,
) -> String {
    let Some(address) = address else {
        return String::new();
    };
    let border_color = escape_html(&brand.accent_color);
    let muted_text_color = escape_html(&brand.muted_text_color);
    let text_color = escape_html(&brand.text_color);
    let mut lines = vec![
        format!("<strong>{}</strong>", escape_html(address.full_name())),
        escape_html(address.address_line1()),
    ];
    if let Some(line2) = address.address_line2() {
        lines.push(escape_html(line2));
    }
    lines.push(escape_html(&address_locality_line(address)));
    lines.push(escape_html(address.country_code()));
    format!(
        "<table role=\"presentation\" width=\"100%\" cellspacing=\"0\" cellpadding=\"0\" style=\"margin:0 0 24px;border:1px solid {border_color};border-radius:8px\"><tr><td style=\"padding:12px 16px;color:{muted_text_color};border-bottom:1px solid {border_color};font-weight:600\">Shipping address</td></tr><tr><td style=\"padding:12px 16px;color:{text_color};font-size:14px;\">{}</td></tr></table>",
        lines.join("<br />")
    )
}

fn address_locality_line(address: &PostalAddress) -> String {
    match (address.administrative_area(), address.postal_code()) {
        (Some(area), Some(postal_code)) => {
            format!("{}, {} {}", address.locality(), area, postal_code)
        }
        (Some(area), None) => format!("{}, {}", address.locality(), area),
        (None, Some(postal_code)) => format!("{}, {}", address.locality(), postal_code),
        (None, None) => address.locality().to_owned(),
    }
}

fn render_support_html(brand: &EmailBrandConfiguration) -> String {
    let mut links = Vec::new();
    if let Some(email) = brand.support_email.as_deref() {
        let email = escape_html(email);
        links.push(format!(
            "<a href=\"mailto:{email}\" style=\"color:{};\">{email}</a>",
            escape_html(&brand.primary_color)
        ));
    }
    if let Some(url) = brand.support_url.as_deref() {
        links.push(format!(
            "<a href=\"{}\" style=\"color:{};\">Help center</a>",
            escape_html(url),
            escape_html(&brand.primary_color),
        ));
    }
    if links.is_empty() {
        "Reply to this email if you need help.".into()
    } else {
        format!("Need help? {}", links.join(" · "))
    }
}

fn render_support_text(brand: &EmailBrandConfiguration) -> String {
    match (brand.support_email.as_deref(), brand.support_url.as_deref()) {
        (Some(email), Some(url)) => format!("Need help? Email {email} or visit {url}"),
        (Some(email), None) => format!("Need help? Email {email}"),
        (None, Some(url)) => format!("Need help? Visit {url}"),
        (None, None) => "Reply to this email if you need help.".into(),
    }
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
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
    use crate::contracts::{EmailBrandConfiguration, EmailOrderLineItem};
    use chaos_domain::sales::PostalAddress;

    use super::{
        OrderConfirmationTemplateData, default_order_confirmation_template,
        render_order_confirmation,
    };

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
        let rendered = render_order_confirmation(
            &default_order_confirmation_template(),
            &OrderConfirmationTemplateData {
                order_number: "ORD-<42>",
                subtotal_amount_minor: 1300,
                discount_amount_minor: 100,
                tax_amount_minor: 50,
                shipping_amount_minor: 99,
                total_amount_minor: 1349,
                currency: "USD",
                lookup_url: "https://shop.example/orders/lookup?order_number=W-1&email=a&b",
                brand: &EmailBrandConfiguration {
                    brand_name: "A <Store>".into(),
                    logo_url: Some("https://cdn.example/logo?a=1&b=2".into()),
                    ..EmailBrandConfiguration::defaults("Fallback".into())
                },
                line_items: &[EmailOrderLineItem {
                    product_title: "T-shirt <classic>".into(),
                    variant_title: "Blue / M".into(),
                    sku: Some("TS-01".into()),
                    quantity: 2,
                    unit_price_amount_minor: 650,
                    subtotal_amount_minor: 1300,
                }],
                shipping_address: Some(&shipping_address),
            },
        );

        assert_eq!(rendered.subject, "A <Store> · Order ORD-<42> confirmed");
        assert!(rendered.text.contains("T-shirt <classic> / Blue / M"));
        assert!(rendered.text.contains("13.00 USD"));
        assert!(rendered.text.contains("Subtotal: 13.00 USD"));
        assert!(rendered.text.contains("Discount: -1.00 USD"));
        assert!(rendered.text.contains("Shipping: 0.99 USD"));
        assert!(rendered.text.contains("Tax: 0.50 USD"));
        assert!(rendered.text.contains("Total: 13.49 USD"));
        assert!(rendered.text.contains(
            "Shipping address:\nBuyer & Co.\n1 Market <Street>\nSuite 42\nSan Francisco, CA 94105\nUS"
        ));
        assert!(rendered.html.contains("A &lt;Store&gt;"));
        assert!(rendered.html.contains("T-shirt &lt;classic&gt;"));
        assert!(rendered.html.contains("13.49 USD"));
        assert!(rendered.html.contains("Subtotal"));
        assert!(rendered.html.contains("Discount"));
        assert!(rendered.html.contains("0.99 USD"));
        assert!(rendered.html.contains("0.50 USD"));
        assert!(rendered.html.contains("Shipping address"));
        assert!(rendered.html.contains("Buyer &amp; Co."));
        assert!(rendered.html.contains("1 Market &lt;Street&gt;"));
        assert!(
            rendered
                .html
                .contains("https://cdn.example/logo?a=1&amp;b=2")
        );
        assert!(
            rendered
                .html
                .contains("https://shop.example/orders/lookup?order_number=W-1&amp;email=a&amp;b")
        );
        assert!(!rendered.html.contains("T-shirt <classic>"));
    }

    #[test]
    fn renders_a_fallback_when_an_order_has_no_line_items_or_support_contact() {
        let rendered = render_order_confirmation(
            &default_order_confirmation_template(),
            &OrderConfirmationTemplateData {
                order_number: "ORD-42",
                subtotal_amount_minor: 1299,
                discount_amount_minor: 0,
                tax_amount_minor: 0,
                shipping_amount_minor: 0,
                total_amount_minor: 1299,
                currency: "USD",
                lookup_url: "https://shop.example/lookup",
                brand: &EmailBrandConfiguration::defaults("Example Store".into()),
                line_items: &[],
                shipping_address: None,
            },
        );

        assert!(rendered.text.contains("No item details available."));
        assert!(
            rendered
                .text
                .contains("Reply to this email if you need help.")
        );
        assert!(rendered.html.contains("No item details available."));
        assert!(!rendered.text.contains("Shipping address:"));
        assert!(!rendered.html.contains("Shipping address"));
        assert!(!rendered.text.contains("Discount:"));
        assert!(!rendered.html.contains(">Discount</td>"));
    }

    #[test]
    fn formats_zero_and_three_decimal_currencies_without_float_rounding() {
        let rendered = render_order_confirmation(
            &default_order_confirmation_template(),
            &OrderConfirmationTemplateData {
                order_number: "ORD-43",
                subtotal_amount_minor: 1234,
                discount_amount_minor: 0,
                tax_amount_minor: 0,
                shipping_amount_minor: 0,
                total_amount_minor: 1234,
                currency: "JPY",
                lookup_url: "https://shop.example/lookup",
                brand: &EmailBrandConfiguration::defaults("Example Store".into()),
                line_items: &[EmailOrderLineItem {
                    product_title: "Coffee".into(),
                    variant_title: "250g".into(),
                    sku: None,
                    quantity: 1,
                    unit_price_amount_minor: 1234,
                    subtotal_amount_minor: 1234,
                }],
                shipping_address: None,
            },
        );
        assert!(rendered.text.contains("Total: 1234 JPY"));
        assert!(rendered.text.contains("1234 JPY"));

        let rendered = render_order_confirmation(
            &default_order_confirmation_template(),
            &OrderConfirmationTemplateData {
                order_number: "ORD-44",
                subtotal_amount_minor: 1234,
                discount_amount_minor: 0,
                tax_amount_minor: 0,
                shipping_amount_minor: 0,
                total_amount_minor: 1234,
                currency: "KWD",
                lookup_url: "https://shop.example/lookup",
                brand: &EmailBrandConfiguration::defaults("Example Store".into()),
                line_items: &[],
                shipping_address: None,
            },
        );
        assert!(rendered.text.contains("Total: 1.234 KWD"));
    }

    #[test]
    fn does_not_reinterpret_placeholder_text_inside_replacements() {
        let rendered = super::render_template(
            "{{first}}/{{second}}",
            &[("first", "{{second}}"), ("second", "resolved")],
        );
        assert_eq!(rendered, "{{second}}/resolved");
    }
}
