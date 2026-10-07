use std::{net::SocketAddr, path::PathBuf};

use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::Html,
    routing::get,
};
use chaos_core::{
    contracts::{
        EmailBrandConfiguration, EmailOrderLineItem, FulfillmentEmailData, FulfillmentEmailStatus,
        OrderConfirmationEmailData,
    },
    email_templates::{EmailTemplateError, EmailTemplateRenderer, RenderedEmailTemplate},
};
use chaos_domain::sales::PostalAddress;

const SCENARIOS: [&str; 6] = [
    "order-full",
    "order-minimal",
    "order-empty",
    "shipped-tracked",
    "shipped-untracked",
    "delivered",
];

#[derive(Clone)]
struct PreviewState {
    template_root: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let state = PreviewState {
        template_root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates/email"),
    };
    let renderer = EmailTemplateRenderer::from_directory(&state.template_root)?;
    for scenario in SCENARIOS {
        render_scenario(&renderer, scenario)?;
    }

    let app = Router::new()
        .route("/", get(gallery))
        .route("/preview/{scenario}", get(preview))
        .route("/plain/{scenario}", get(plain))
        .with_state(state);
    let address = SocketAddr::from(([127, 0, 0, 1], 3100));
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("Email preview: http://{address}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn gallery() -> Html<&'static str> {
    Html(GALLERY)
}

async fn preview(
    State(state): State<PreviewState>,
    Path(scenario): Path<String>,
) -> Result<Html<String>, (StatusCode, String)> {
    render(&state, &scenario).map(|message| Html(message.html))
}

async fn plain(
    State(state): State<PreviewState>,
    Path(scenario): Path<String>,
) -> Result<Html<String>, (StatusCode, String)> {
    render(&state, &scenario).map(|message| {
        Html(format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><style>body{{margin:0;padding:32px;background:#f4f6f8;color:#17202a;font:14px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace}}section{{max-width:720px;margin:0 auto 20px;background:white;border:1px solid #d0d5dd;border-radius:10px;padding:20px}}h1{{font:600 13px/1.4 system-ui,sans-serif;color:#667085;text-transform:uppercase;letter-spacing:.06em}}pre{{white-space:pre-wrap;overflow-wrap:anywhere}}</style></head><body><section><h1>Subject</h1><pre>{}</pre></section><section><h1>Plain text</h1><pre>{}</pre></section></body></html>",
            escape_html(&message.subject),
            escape_html(&message.text),
        ))
    })
}

fn render(
    state: &PreviewState,
    scenario: &str,
) -> Result<RenderedEmailTemplate, (StatusCode, String)> {
    let renderer = EmailTemplateRenderer::from_directory(&state.template_root)
        .map_err(internal_preview_error)?;
    render_scenario(&renderer, scenario).map_err(|error| match error {
        PreviewError::UnknownScenario => (
            StatusCode::NOT_FOUND,
            format!("unknown email preview scenario: {scenario}"),
        ),
        PreviewError::Template(error) => internal_preview_error(error),
    })
}

fn render_scenario(
    renderer: &EmailTemplateRenderer,
    scenario: &str,
) -> Result<RenderedEmailTemplate, PreviewError> {
    match scenario {
        "order-full" => renderer
            .render_order_confirmation(&full_order())
            .map_err(Into::into),
        "order-minimal" => renderer
            .render_order_confirmation(&minimal_order())
            .map_err(Into::into),
        "order-empty" => renderer
            .render_order_confirmation(&empty_order())
            .map_err(Into::into),
        "shipped-tracked" => renderer
            .render_fulfillment_update(&fulfillment(
                FulfillmentEmailStatus::Shipped,
                Some("1Z999AA10123456784"),
                Some("https://tracking.example.test/1Z999AA10123456784"),
            ))
            .map_err(Into::into),
        "shipped-untracked" => renderer
            .render_fulfillment_update(&fulfillment(FulfillmentEmailStatus::Shipped, None, None))
            .map_err(Into::into),
        "delivered" => renderer
            .render_fulfillment_update(&fulfillment(
                FulfillmentEmailStatus::Delivered,
                Some("ignored-for-delivered"),
                Some("https://tracking.example.test/ignored"),
            ))
            .map_err(Into::into),
        _ => Err(PreviewError::UnknownScenario),
    }
}

#[derive(Debug, thiserror::Error)]
enum PreviewError {
    #[error("unknown preview scenario")]
    UnknownScenario,
    #[error(transparent)]
    Template(#[from] EmailTemplateError),
}

fn internal_preview_error(error: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

fn full_order() -> OrderConfirmationEmailData {
    OrderConfirmationEmailData {
        order_number: "W-20261007-7K4M9Q2D".into(),
        subtotal_amount_minor: 18_900,
        discount_amount_minor: 2_000,
        tax_amount_minor: 1_352,
        shipping_amount_minor: 1_200,
        total_amount_minor: 19_452,
        currency: "USD".into(),
        lookup_url: "https://shop.example.test/orders/details?order_number=W-20261007-7K4M9Q2D&email=buyer%40example.test".into(),
        brand: preview_brand(),
        line_items: vec![
            EmailOrderLineItem {
                product_title: "Everyday Canvas Backpack".into(),
                variant_title: "Ocean blue / 18 L".into(),
                sku: Some("BAG-OCEAN-18".into()),
                quantity: 1,
                unit_price_amount_minor: 12_900,
                subtotal_amount_minor: 12_900,
                image_url: Some(
                    "https://placehold.co/88x88/EFF4FF/175CD3.png?text=Bag".into(),
                ),
            },
            EmailOrderLineItem {
                product_title: "Insulated Bottle".into(),
                variant_title: "Silver / 750 ml".into(),
                sku: Some("BOT-SILVER-750".into()),
                quantity: 2,
                unit_price_amount_minor: 3_000,
                subtotal_amount_minor: 6_000,
                image_url: Some(
                    "https://placehold.co/88x88/ECFDF3/027A48.png?text=Bottle".into(),
                ),
            },
        ],
        shipping_address: Some(
            PostalAddress::new(
                "Alex Morgan",
                "18 Marina View",
                Some("#24-03".into()),
                "Singapore",
                None,
                Some("018987".into()),
                "SG",
            )
            .expect("preview address is valid"),
        ),
    }
}

fn minimal_order() -> OrderConfirmationEmailData {
    OrderConfirmationEmailData {
        order_number: "W-20261007-4B2M8N1P".into(),
        subtotal_amount_minor: 4_800,
        discount_amount_minor: 0,
        tax_amount_minor: 0,
        shipping_amount_minor: 0,
        total_amount_minor: 4_800,
        currency: "USD".into(),
        lookup_url: "https://shop.example.test/orders/details".into(),
        brand: EmailBrandConfiguration::defaults("Northstar Supply".into()),
        line_items: vec![EmailOrderLineItem {
            product_title: "Field Notes Set".into(),
            variant_title: "Kraft / Set of 3".into(),
            sku: None,
            quantity: 1,
            unit_price_amount_minor: 4_800,
            subtotal_amount_minor: 4_800,
            image_url: None,
        }],
        shipping_address: None,
    }
}

fn empty_order() -> OrderConfirmationEmailData {
    OrderConfirmationEmailData {
        line_items: Vec::new(),
        ..minimal_order()
    }
}

fn fulfillment(
    status: FulfillmentEmailStatus,
    tracking_number: Option<&str>,
    tracking_url: Option<&str>,
) -> FulfillmentEmailData {
    FulfillmentEmailData {
        order_number: "W-20261007-7K4M9Q2D".into(),
        status,
        tracking_number: tracking_number.map(str::to_owned),
        tracking_url: tracking_url.map(str::to_owned),
        lookup_url: "https://shop.example.test/orders/details?order_number=W-20261007-7K4M9Q2D"
            .into(),
        brand: preview_brand(),
    }
}

fn preview_brand() -> EmailBrandConfiguration {
    EmailBrandConfiguration {
        brand_name: "Northstar Supply".into(),
        logo_url: Some("https://placehold.co/72x72/175CD3/FFFFFF.png?text=N".into()),
        primary_color: "#175CD3".into(),
        accent_color: "#D0D5DD".into(),
        background_color: "#F2F4F7".into(),
        surface_color: "#FFFFFF".into(),
        text_color: "#101828".into(),
        muted_text_color: "#667085".into(),
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

const GALLERY: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>Chaos email previews</title>
  <style>
    *{box-sizing:border-box}body{margin:0;background:#f2f4f7;color:#101828;font:14px/1.45 Inter,ui-sans-serif,system-ui,sans-serif}
    header{height:64px;padding:0 24px;background:#fff;border-bottom:1px solid #e4e7ec;display:flex;align-items:center;justify-content:space-between}
    h1{margin:0;font-size:17px}.hint{color:#667085;font-size:12px}.shell{display:grid;grid-template-columns:240px 1fr;min-height:calc(100vh - 64px)}
    nav{padding:20px;background:#fff;border-right:1px solid #e4e7ec}nav h2{margin:0 0 8px;color:#667085;font-size:11px;text-transform:uppercase;letter-spacing:.08em}
    nav button{display:block;width:100%;margin:0 0 4px;padding:9px 10px;border:0;border-radius:6px;background:transparent;color:#344054;text-align:left;cursor:pointer}
    nav button:hover,nav button.active{background:#eff4ff;color:#175cd3}.workspace{min-width:0;padding:20px}.toolbar{display:flex;gap:8px;align-items:center;margin:0 0 16px}
    .toolbar button{padding:7px 12px;border:1px solid #d0d5dd;border-radius:6px;background:#fff;cursor:pointer}.toolbar button.active{border-color:#175cd3;color:#175cd3;background:#eff4ff}
    .frame{margin:auto;background:#fff;border:1px solid #d0d5dd;border-radius:12px;box-shadow:0 8px 24px #10182812;overflow:hidden;transition:width .2s}.frame.desktop{width:min(100%,720px)}.frame.mobile{width:375px;max-width:100%}
    iframe{display:block;width:100%;height:780px;border:0;background:#fff}@media(max-width:720px){.shell{grid-template-columns:1fr}nav{border-right:0;border-bottom:1px solid #e4e7ec}.hint{display:none}}
  </style>
</head>
<body>
  <header><h1>Transactional email previews</h1><span class="hint">Templates reload on every browser refresh</span></header>
  <div class="shell">
    <nav>
      <h2>Order confirmation</h2>
      <button data-scenario="order-full">Full order</button>
      <button data-scenario="order-minimal">Minimal order</button>
      <button data-scenario="order-empty">Empty items fallback</button>
      <h2 style="margin-top:20px">Fulfillment</h2>
      <button data-scenario="shipped-tracked">Shipped with tracking</button>
      <button data-scenario="shipped-untracked">Shipped without tracking</button>
      <button data-scenario="delivered">Delivered</button>
    </nav>
    <main class="workspace">
      <div class="toolbar">
        <button data-view="html" class="active">HTML</button><button data-view="plain">Subject + text</button>
        <span style="flex:1"></span>
        <button data-size="desktop" class="active">Desktop</button><button data-size="mobile">Mobile</button>
      </div>
      <div id="frame" class="frame desktop"><iframe id="preview" title="Email preview"></iframe></div>
    </main>
  </div>
  <script>
    let scenario='order-full',view='html';const iframe=document.querySelector('#preview'),frame=document.querySelector('#frame');
    function update(){iframe.src=(view==='html'?'/preview/':'/plain/')+scenario;document.querySelectorAll('[data-scenario]').forEach(x=>x.classList.toggle('active',x.dataset.scenario===scenario));document.querySelectorAll('[data-view]').forEach(x=>x.classList.toggle('active',x.dataset.view===view))}
    document.querySelectorAll('[data-scenario]').forEach(x=>x.onclick=()=>{scenario=x.dataset.scenario;update()});
    document.querySelectorAll('[data-view]').forEach(x=>x.onclick=()=>{view=x.dataset.view;update()});
    document.querySelectorAll('[data-size]').forEach(x=>x.onclick=()=>{frame.className='frame '+x.dataset.size;document.querySelectorAll('[data-size]').forEach(y=>y.classList.toggle('active',y===x))});update();
  </script>
</body>
</html>"#;

#[cfg(test)]
mod tests {
    use super::{EmailTemplateRenderer, PathBuf, SCENARIOS, render_scenario};

    #[test]
    fn every_preview_scenario_renders_from_disk() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates/email");
        let renderer = EmailTemplateRenderer::from_directory(root).unwrap();
        for scenario in SCENARIOS {
            let message = render_scenario(&renderer, scenario).unwrap();
            assert!(message.html.starts_with("<!doctype html>"));
            assert!(!message.html.contains("{{"));
            assert!(!message.html.contains("{%"));
        }
    }
}
