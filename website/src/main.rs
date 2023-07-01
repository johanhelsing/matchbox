use stylist::{global_style, yew::styled_component};
use yew::{html, Html};

#[styled_component(Root)]
fn root() -> Html {
    global_style!(
        r#"
        html {
            min-height: 100%;
            position: relative;
        }
        
        body {
            height: 100%;
            padding: 0;
            margin: 0;
        }
    "#
    )
    .expect("Unable to mount global style");

    let css = css!(
        r#"
        position: absolute;
        overflow: hidden;
        width: 100%;
        height: 100%;
        "#
    );

    html! {
        <div class={ css }>
            <canvas id="bevy_ggrs_example"></canvas>
        </div>

    }
}

fn main() {
    yew::Renderer::<Root>::new().render();
    bevy_ggrs_example_lib::run();
}
