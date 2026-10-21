<script>
  let active = false;
  const label = '<div class="in-script">';
</script>

<div class="btn">one</div>                                    <!--~ btn -->
<div class='btn btn-large'>two</div>                          <!--~ btn, btn-large -->
<div class="btn {active ? 'card' : ''} page">three</div>      <!--~ btn, card, page -->
<div class={active ? 'on' : 'off'}>four</div>                 <!--~ on, off -->
<div class={['home', active && 'btn-large']}>five</div>       <!--~ home, btn-large -->
<div class={{ active, 'is-big': big, spaced: x }}>six</div>   <!--~ active, is-big, spaced -->
<button class:active>seven</button>                           <!--~ active -->
<button class:btn-large={big}>eight</button>                  <!--~ btn-large -->
<button class:title={active} class="x">nine</button>          <!--~ title, x -->
<Component class="comp {inner}" />                            <!--~ comp -->
<div class={`tpl ${active ? 'a1' : 'a2'}`}>ten</div>          <!--~ tpl, a1, a2 -->
