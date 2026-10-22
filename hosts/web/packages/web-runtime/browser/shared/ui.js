import{defineComponent as e,h as t}from"vue";function n(e=`dark`){let t=document.documentElement;e===`light`?t.dataset.theme=`light`:delete t.dataset.theme,t.style.colorScheme=e}function r(e){return e===`light`||e===`dark`?e:typeof window<`u`&&window.matchMedia&&window.matchMedia(`(prefers-color-scheme: light)`).matches?`light`:`dark`}var i=e({name:`MutsukiConsoleShell`,props:{brand:{type:String,default:`Mutsuki`},footer:{type:String,default:`Bot 控制台`},title:{type:String,default:``},subtitle:{type:String,default:``},navItems:{type:Array,default:()=>[]}},emits:[`navigate`,`refresh`],setup(e,{emit:n,slots:r}){return()=>t(`div`,{class:`mutsuki-console lilia-workspace`,"data-lilia-surface-mode":`solid`,"data-lilia-surface-level":`base`},[t(`aside`,{class:`lilia-workspace-region`,"data-region":`navigation`,"data-region-separator":`inline`},[t(`div`,{class:`secondary-panel`},[t(`div`,{class:`secondary-panel__top`},[r.brand?.()??t(`div`,{class:`brand`},e.brand)]),r.nav?.()??t(`nav`,{class:`secondary-panel__body sb-section nav`,"aria-label":`控制台`},e.navItems.map(e=>{let r=[`sb-tree__row`,`lilia-interactive-item`,e.active?`is-active`:void 0],i=[t(`span`,{class:`sb-tree__name`},e.label)];return e.href?t(`a`,{class:r,href:e.href,"aria-current":e.active?`page`:void 0},i):t(`button`,{type:`button`,class:r,"aria-current":e.active?`page`:void 0,onClick:()=>n(`navigate`,e.id)},i)})),r.footer?.()??t(`div`,{class:`secondary-panel__footer sidebar-footer`},e.footer)])]),t(`main`,{class:`lilia-workspace-region`,"data-region":`main`},[t(`div`,{class:`lilia-workspace-region__content page-scroll`},[r.header?.()??t(`div`,{class:`page-header`},[t(`div`,[t(`h1`,e.title),e.subtitle?t(`p`,e.subtitle):null]),t(`div`,{class:`page-actions`},[t(`button`,{type:`button`,class:`ghost`,onClick:()=>n(`refresh`)},`刷新`)])]),t(`section`,{class:`page-body`,id:`content`},r.default?.()??[])])])])}});function a(e){let t=document.createElement(`div`);return t.className=`mutsuki-console lilia-workspace`,t.dataset.liliaSurfaceMode=`solid`,t.dataset.liliaSurfaceLevel=`base`,t.innerHTML=`
    <aside class="lilia-workspace-region" data-region="navigation" data-region-separator="inline">
      <div class="secondary-panel">
        <div class="secondary-panel__top">
          <div class="brand">${o(e.brand??`Mutsuki`)}</div>
        </div>
        <nav class="secondary-panel__body sb-section nav" aria-label="控制台"></nav>
        <div class="secondary-panel__footer sidebar-footer">${o(e.footer??`Bot 控制台`)}</div>
      </div>
    </aside>
    <main class="lilia-workspace-region" data-region="main">
      <div class="lilia-workspace-region__content page-scroll">
        <div class="page-header">
          <div>
            <h1 id="page-title">${o(e.title??``)}</h1>
            <p id="page-subtitle">${o(e.subtitle??``)}</p>
          </div>
          ${e.showRefresh===!1?``:`<div class="page-actions"><button type="button" id="refresh" class="ghost">刷新</button></div>`}
        </div>
        <section id="content" class="page-body"></section>
      </div>
    </main>
  `,t}function o(e){return e.replaceAll(`&`,`&amp;`).replaceAll(`<`,`&lt;`).replaceAll(`>`,`&gt;`).replaceAll(`"`,`&quot;`)}export{i as ConsoleShell,n as applyTheme,a as createConsoleShellElement,r as resolveTheme};