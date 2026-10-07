use std::{net::SocketAddr, path::PathBuf};

use axum::{
    Router,
    extract::{Path, Query, State},
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
use serde::Deserialize;

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

#[derive(Debug, Default, Deserialize)]
struct PreviewThemeQuery {
    brand: Option<String>,
    logo: Option<String>,
    primary: Option<String>,
    accent: Option<String>,
    background: Option<String>,
    surface: Option<String>,
    text: Option<String>,
    muted: Option<String>,
}

impl PreviewThemeQuery {
    fn resolve(&self) -> EmailBrandConfiguration {
        let mut brand = preview_brand();
        if let Some(value) = self
            .brand
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            brand.brand_name = value.trim().to_owned();
        }
        if let Some(value) = &self.logo {
            brand.logo_url = (!value.trim().is_empty()).then(|| value.trim().to_owned());
        }
        assign_color(&mut brand.primary_color, self.primary.as_deref());
        assign_color(&mut brand.accent_color, self.accent.as_deref());
        assign_color(&mut brand.background_color, self.background.as_deref());
        assign_color(&mut brand.surface_color, self.surface.as_deref());
        assign_color(&mut brand.text_color, self.text.as_deref());
        assign_color(&mut brand.muted_text_color, self.muted.as_deref());
        brand
    }
}

fn assign_color(target: &mut String, value: Option<&str>) {
    if let Some(value) = value.filter(|value| is_hex_color(value)) {
        *target = value.to_ascii_uppercase();
    }
}

fn is_hex_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let state = PreviewState {
        template_root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates/email"),
    };
    let renderer = EmailTemplateRenderer::from_directory(&state.template_root)?;
    let brand = preview_brand();
    for scenario in SCENARIOS {
        render_scenario(&renderer, scenario, &brand)?;
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
    Query(theme): Query<PreviewThemeQuery>,
) -> Result<Html<String>, (StatusCode, String)> {
    render(&state, &scenario, &theme).map(|message| Html(message.html))
}

async fn plain(
    State(state): State<PreviewState>,
    Path(scenario): Path<String>,
    Query(theme): Query<PreviewThemeQuery>,
) -> Result<Html<String>, (StatusCode, String)> {
    render(&state, &scenario, &theme).map(|message| {
        Html(format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><style>body{{margin:0;padding:32px;background:#f4f6f8;color:#17202a;font:14px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace}}section{{max-width:720px;margin:0 auto 20px;background:white;border:1px solid #d0d5dd;border-radius:10px;padding:20px}}h1{{font:600 13px/1.4 system-ui,sans-serif;color:#667085}}pre{{white-space:pre-wrap;overflow-wrap:anywhere}}</style></head><body><section><h1>Subject</h1><pre>{}</pre></section><section><h1>Plain text</h1><pre>{}</pre></section></body></html>",
            escape_html(&message.subject),
            escape_html(&message.text),
        ))
    })
}

fn render(
    state: &PreviewState,
    scenario: &str,
    theme: &PreviewThemeQuery,
) -> Result<RenderedEmailTemplate, (StatusCode, String)> {
    let renderer = EmailTemplateRenderer::from_directory(&state.template_root)
        .map_err(internal_preview_error)?;
    let brand = theme.resolve();
    render_scenario(&renderer, scenario, &brand).map_err(|error| match error {
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
    brand: &EmailBrandConfiguration,
) -> Result<RenderedEmailTemplate, PreviewError> {
    match scenario {
        "order-full" => renderer
            .render_order_confirmation(&full_order(brand))
            .map_err(Into::into),
        "order-minimal" => renderer
            .render_order_confirmation(&minimal_order(brand))
            .map_err(Into::into),
        "order-empty" => renderer
            .render_order_confirmation(&empty_order(brand))
            .map_err(Into::into),
        "shipped-tracked" => renderer
            .render_fulfillment_update(&fulfillment(
                FulfillmentEmailStatus::Shipped,
                Some("1Z999AA10123456784"),
                Some("https://tracking.example.test/1Z999AA10123456784"),
                brand,
            ))
            .map_err(Into::into),
        "shipped-untracked" => renderer
            .render_fulfillment_update(&fulfillment(
                FulfillmentEmailStatus::Shipped,
                None,
                None,
                brand,
            ))
            .map_err(Into::into),
        "delivered" => renderer
            .render_fulfillment_update(&fulfillment(
                FulfillmentEmailStatus::Delivered,
                Some("ignored-for-delivered"),
                Some("https://tracking.example.test/ignored"),
                brand,
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

fn full_order(brand: &EmailBrandConfiguration) -> OrderConfirmationEmailData {
    OrderConfirmationEmailData {
        recipient_name: Some("Alex Morgan".into()),
        order_number: "W-20261007-7K4M9Q2D".into(),
        subtotal_amount_minor: 18_900,
        discount_amount_minor: 2_000,
        tax_amount_minor: 1_352,
        shipping_amount_minor: 1_200,
        total_amount_minor: 19_452,
        currency: "USD".into(),
        lookup_url: "https://shop.example.test/orders/details?order_number=W-20261007-7K4M9Q2D&email=buyer%40example.test".into(),
        brand: brand.clone(),
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

fn minimal_order(brand: &EmailBrandConfiguration) -> OrderConfirmationEmailData {
    OrderConfirmationEmailData {
        recipient_name: None,
        order_number: "W-20261007-4B2M8N1P".into(),
        subtotal_amount_minor: 4_800,
        discount_amount_minor: 0,
        tax_amount_minor: 0,
        shipping_amount_minor: 0,
        total_amount_minor: 4_800,
        currency: "USD".into(),
        lookup_url: "https://shop.example.test/orders/details".into(),
        brand: brand.clone(),
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

fn empty_order(brand: &EmailBrandConfiguration) -> OrderConfirmationEmailData {
    OrderConfirmationEmailData {
        line_items: Vec::new(),
        ..minimal_order(brand)
    }
}

fn fulfillment(
    status: FulfillmentEmailStatus,
    tracking_number: Option<&str>,
    tracking_url: Option<&str>,
    brand: &EmailBrandConfiguration,
) -> FulfillmentEmailData {
    FulfillmentEmailData {
        recipient_name: Some("Alex Morgan".into()),
        order_number: "W-20261007-7K4M9Q2D".into(),
        status,
        tracking_number: tracking_number.map(str::to_owned),
        tracking_url: tracking_url.map(str::to_owned),
        lookup_url: "https://shop.example.test/orders/details?order_number=W-20261007-7K4M9Q2D"
            .into(),
        brand: brand.clone(),
    }
}

fn preview_brand() -> EmailBrandConfiguration {
    EmailBrandConfiguration {
        brand_name: "Northstar Supply".into(),
        logo_url: Some("https://placehold.co/64x64/0071E3/FFFFFF.png?text=N".into()),
        primary_color: "#0071E3".into(),
        accent_color: "#D2D2D7".into(),
        background_color: "#F5F5F7".into(),
        surface_color: "#FFFFFF".into(),
        text_color: "#1D1D1F".into(),
        muted_text_color: "#6E6E73".into(),
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
    :root{color-scheme:light}*{box-sizing:border-box}body{margin:0;background:#f5f5f7;color:#1d1d1f;font:13px/1.45 -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}
    header{height:60px;padding:0 22px;background:#fff;border-bottom:1px solid #dedbd6;display:flex;align-items:center;justify-content:space-between}
    h1{margin:0;font-size:16px;font-weight:600;line-height:1.2}.hint{color:#6e6e73;font-size:12px}.shell{display:grid;grid-template-columns:280px minmax(0,1fr);min-height:calc(100vh - 60px)}
    aside{padding:20px;background:#fff;border-right:1px solid #d2d2d7;overflow:auto}fieldset{margin:0 0 24px;padding:0;border:0}legend{width:100%;margin:0 0 9px;color:#6e6e73;font-size:11px;font-weight:600}
    .scenario{display:block;width:100%;margin:0 0 3px;padding:8px 9px;border:0;border-radius:6px;background:transparent;color:#3a3a3c;text-align:left;cursor:pointer}.scenario:hover,.scenario.active{background:#f5f5f7;color:#0071e3}
    label{display:block;margin:0 0 10px;color:#57534e;font-size:11px;font-weight:600}.text-input,select{display:block;width:100%;height:34px;margin-top:4px;padding:0 9px;border:1px solid #d6d3d1;border-radius:4px;background:#fff;color:#292524;font:12px inherit}
    .color-row{display:grid;grid-template-columns:1fr 28px 64px;gap:7px;align-items:center;margin:0 0 8px;color:#57534e;font-size:11px}.color-row input{width:28px;height:28px;padding:2px;border:1px solid #d6d3d1;border-radius:4px;background:#fff}.color-row code{color:#78716c;font-size:10px;text-align:right}
    .reset{width:100%;padding:7px;border:1px solid #d6d3d1;border-radius:4px;background:#fff;color:#57534e;cursor:pointer}.workspace{min-width:0;padding:18px 22px 32px}.toolbar{display:flex;gap:6px;align-items:center;margin:0 0 14px}
    .toolbar button{padding:6px 10px;border:1px solid #d2d2d7;border-radius:6px;background:#fff;color:#3a3a3c;cursor:pointer}.toolbar button.active{border-color:#0071e3;color:#0071e3;background:#f5f9ff}
    .frame{margin:auto;background:#fff;border:1px solid #d6d3d1;box-shadow:0 10px 28px #29252412;overflow:hidden;transition:width .2s}.frame.desktop{width:min(100%,680px)}.frame.mobile{width:375px;max-width:100%}
    iframe{display:block;width:100%;height:860px;border:0;background:#fff}@media(max-width:780px){.shell{grid-template-columns:1fr}aside{border-right:0;border-bottom:1px solid #dedbd6}.hint{display:none}.workspace{padding:14px 10px 24px}}
  </style>
</head>
<body>
  <header><h1>Transactional email studio</h1><span class="hint">Shared variables update every scenario</span></header>
  <div class="shell">
    <aside>
      <fieldset>
        <legend>Order confirmation</legend>
        <button class="scenario" data-scenario="order-full">Full order</button>
        <button class="scenario" data-scenario="order-minimal">Minimal order</button>
        <button class="scenario" data-scenario="order-empty">Empty items fallback</button>
      </fieldset>
      <fieldset>
        <legend>Fulfillment</legend>
        <button class="scenario" data-scenario="shipped-tracked">Shipped with tracking</button>
        <button class="scenario" data-scenario="shipped-untracked">Shipped without tracking</button>
        <button class="scenario" data-scenario="delivered">Delivered</button>
      </fieldset>
      <fieldset id="theme-controls">
        <legend>Shared variables</legend>
        <label>Preset<select id="preset"><option value="system">System light</option><option value="ink">Ink &amp; paper</option><option value="forest">Evergreen</option><option value="warm">Warm neutral</option><option value="custom">Custom</option></select></label>
        <label>Brand name<input class="text-input" id="brand" value="Northstar Supply"></label>
        <label>Logo URL<input class="text-input" id="logo" value="https://placehold.co/64x64/0071E3/FFFFFF.png?text=N"></label>
        <div class="color-row"><span>Primary</span><input type="color" id="primary"><code data-value="primary"></code></div>
        <div class="color-row"><span>Accent</span><input type="color" id="accent"><code data-value="accent"></code></div>
        <div class="color-row"><span>Background</span><input type="color" id="background"><code data-value="background"></code></div>
        <div class="color-row"><span>Surface</span><input type="color" id="surface"><code data-value="surface"></code></div>
        <div class="color-row"><span>Text</span><input type="color" id="text"><code data-value="text"></code></div>
        <div class="color-row"><span>Muted text</span><input type="color" id="muted"><code data-value="muted"></code></div>
        <button class="reset" id="reset" type="button">Reset variables</button>
      </fieldset>
    </aside>
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
    const presets={
      system:{primary:'#0071E3',accent:'#D2D2D7',background:'#F5F5F7',surface:'#FFFFFF',text:'#1D1D1F',muted:'#6E6E73'},
      warm:{primary:'#8B5E34',accent:'#DED3C4',background:'#F5F0E8',surface:'#FFFDFC',text:'#292524',muted:'#78716C'},
      ink:{primary:'#303030',accent:'#D6D3D1',background:'#F1F0ED',surface:'#FFFFFF',text:'#1C1917',muted:'#6B6865'},
      forest:{primary:'#285943',accent:'#C8D5CE',background:'#EDF2EE',surface:'#FEFFFE',text:'#1F2923',muted:'#68756D'}
    };
    const colorKeys=['primary','accent','background','surface','text','muted'],iframe=document.querySelector('#preview'),frame=document.querySelector('#frame'),preset=document.querySelector('#preset');
    let scenario='order-full',view='html',timer;
    function setTheme(theme){colorKeys.forEach(key=>document.querySelector('#'+key).value=theme[key]);syncValues()}
    function syncValues(){colorKeys.forEach(key=>document.querySelector('[data-value="'+key+'"]').textContent=document.querySelector('#'+key).value.toUpperCase())}
    function params(){const value=new URLSearchParams({brand:document.querySelector('#brand').value,logo:document.querySelector('#logo').value});colorKeys.forEach(key=>value.set(key,document.querySelector('#'+key).value));return value}
    function update(){iframe.src=(view==='html'?'/preview/':'/plain/')+scenario+'?'+params();document.querySelectorAll('[data-scenario]').forEach(x=>x.classList.toggle('active',x.dataset.scenario===scenario));document.querySelectorAll('[data-view]').forEach(x=>x.classList.toggle('active',x.dataset.view===view));syncValues();localStorage.setItem('chaos-email-theme-v2',JSON.stringify(Object.fromEntries(params())))}
    function schedule(){clearTimeout(timer);timer=setTimeout(update,120)}
    function restore(){let theme=presets.system;try{const saved=JSON.parse(localStorage.getItem('chaos-email-theme-v2'));if(saved){theme={...theme,...saved};document.querySelector('#brand').value=theme.brand||'Northstar Supply';document.querySelector('#logo').value=theme.logo??'';preset.value='custom'}}catch{}setTheme(theme)}
    document.querySelectorAll('[data-scenario]').forEach(x=>x.onclick=()=>{scenario=x.dataset.scenario;update()});
    document.querySelectorAll('[data-view]').forEach(x=>x.onclick=()=>{view=x.dataset.view;update()});
    document.querySelectorAll('[data-size]').forEach(x=>x.onclick=()=>{frame.className='frame '+x.dataset.size;document.querySelectorAll('[data-size]').forEach(y=>y.classList.toggle('active',y===x))});
    preset.onchange=()=>{if(preset.value!=='custom'){setTheme(presets[preset.value]);update()}};
    document.querySelectorAll('#theme-controls input').forEach(x=>x.oninput=()=>{preset.value='custom';schedule()});
    document.querySelector('#reset').onclick=()=>{localStorage.removeItem('chaos-email-theme-v2');document.querySelector('#brand').value='Northstar Supply';document.querySelector('#logo').value='https://placehold.co/64x64/0071E3/FFFFFF.png?text=N';preset.value='system';setTheme(presets.system);update()};
    restore();update();
  </script>
</body>
</html>"#;

#[cfg(test)]
mod tests {
    use super::{
        EmailTemplateRenderer, PathBuf, PreviewThemeQuery, SCENARIOS, preview_brand,
        render_scenario,
    };

    #[test]
    fn every_preview_scenario_renders_from_disk() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates/email");
        let renderer = EmailTemplateRenderer::from_directory(root).unwrap();
        let brand = preview_brand();
        for scenario in SCENARIOS {
            let message = render_scenario(&renderer, scenario, &brand).unwrap();
            assert!(message.html.starts_with("<!doctype html>"));
            assert!(
                message.html.contains("View order"),
                "{scenario} should use the shared primary action"
            );
            assert!(!message.html.contains("{{"));
            assert!(!message.html.contains("{%"));
        }
    }

    #[test]
    fn shared_theme_query_applies_valid_preview_variables() {
        let theme = PreviewThemeQuery {
            brand: Some("Paper & Pine".into()),
            logo: Some(String::new()),
            primary: Some("#285943".into()),
            accent: Some("not-a-color".into()),
            ..PreviewThemeQuery::default()
        }
        .resolve();

        assert_eq!(theme.brand_name, "Paper & Pine");
        assert_eq!(theme.logo_url, None);
        assert_eq!(theme.primary_color, "#285943");
        assert_eq!(theme.accent_color, "#D2D2D7");
    }
}
