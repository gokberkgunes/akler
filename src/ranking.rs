const RANK_SCORE:usize=N_METRICS;
const RANK_COVERAGE:usize=N_METRICS+1;
const RANK_COUNT:usize=N_METRICS+2;
const RANK_Y:usize=3;
const RANK_DATA:usize=6;
const RANK_ORDER:[usize;RANK_COUNT]=[
    RANK_SCORE,SFB,SFS,TRAVEL,SFTRAVEL,FSB,HSB,FSS,HSS,LSB,LSS,REDIR,WRED,WISH,OSF,SRAF,ROLL,
    DFSB,CFSB,DFSS,CFSS,ALT,DSB,CSB,DSS,CSS,VTRAVEL,LTRAVEL,RANK_COVERAGE,
];
fn rank_name(m:usize)->&'static str{if m==RANK_SCORE{"SCORE"}else if m==RANK_COVERAGE{"COVERAGE"}else{METRIC_NAMES[m]}}
fn rank_positive(m:usize)->bool{m==RANK_COVERAGE||(m<N_METRICS&&higher_better(m))}
#[derive(Clone)]
struct RankRow{model:Model,corpus:Corpus,raw:Raw,metrics:Metrics,score:f64}
fn rank_value(row:&RankRow,m:usize)->Option<f64>{
    if m==RANK_SCORE{Some(row.score)}else if m==RANK_COVERAGE{Some(row.corpus.coverage[1])}
    else if denominator(m,&row.raw,&row.corpus)>0.0{Some(row.metrics.v[m])}else{None}
}
fn load_ranking(source:&Source)->AppResult<(Vec<RankRow>,Vec<String>)>{
    let w=load_weights(Path::new(WEIGHTS_FILE))?;let mut rows=Vec::new();let mut errors=Vec::new();
    for path in discover(LAYOUT_DIR,"dat","")?{
        let result=(||->AppResult<RankRow>{let model=Model::new(load_board(&path)?);let corpus=model.corpus(source)?;
            let raw=full_raw(&model.original,&corpus,&model.geometry);let metrics=metrics(&raw,&corpus);let score=breakdown(&metrics,&w).net;
            Ok(RankRow{model,corpus,raw,metrics,score})})();
        match result{Ok(row)=>rows.push(row),Err(e)=>errors.push(e.to_string())}
    }if rows.is_empty(){return Err(format!("no valid layouts\n{}",errors.join("\n")).into());}Ok((rows,errors))
}
fn inspect_row(term:&mut Terminal,row:&RankRow)->AppResult<()> {
    let mut scroll=0;let w=load_weights(Path::new(WEIGHTS_FILE))?;
    loop{let c=dashboard(term,"Layout", "v validate | q back",&row.model,&row.model.original,&row.model.original,&row.corpus,&w,None,None,"",None,false);
        term.present(&c,scroll)?;let e=term.event()?;if scroll_event(&e,&mut scroll,c.h,term.size.1){continue;}
        if let Some(Action::Metric(m))=action_press(term,&c,&e,scroll){contributor_view(term,m,&row.model,&row.model.original,&row.model.original,&row.corpus,false)?;}
        match e{Event::Char('.')=>term.precise=!term.precise,Event::Escape|Event::Quit|Event::Char('q')=>return Ok(()),
            Event::Char('i')=>show_corpus_info(term,&row.corpus)?,Event::Char('v')=>validation_view(term,&row.model,&row.model.original,&row.model.original)?,_=>{}}
    }
}
#[derive(Clone,Debug)]
enum Hidden{Column(usize),Row(PathBuf)}
struct RankColumns{hidden:[bool;RANK_COUNT],rows:BTreeSet<PathBuf>,history:Vec<Hidden>,first:usize}
impl RankColumns{
    fn new()->Self{Self{hidden:[false;RANK_COUNT],rows:BTreeSet::new(),history:Vec::new(),first:0}}
    fn visible(&self)->Vec<usize>{RANK_ORDER.iter().copied().filter(|&m|!self.hidden[m]).collect()}
    fn hide(&mut self,m:usize)->bool{if m>=RANK_COUNT||self.hidden[m]{return false;}
        if let Some(i)=self.visible().iter().position(|&id|id==m){if i<self.first{self.first=self.first.saturating_sub(1);}}
        self.hidden[m]=true;self.history.push(Hidden::Column(m));true}
    fn hide_row(&mut self,path:PathBuf)->bool{if !self.rows.insert(path.clone()){return false;}self.history.push(Hidden::Row(path));true}
    fn undo(&mut self,capacity:usize){if let Some(last)=self.history.pop(){match last{
        Hidden::Row(p)=>{self.rows.remove(&p);},Hidden::Column(m)=>{self.hidden[m]=false;
            if let Some(i)=self.visible().iter().position(|&id|id==m){if i<self.first{self.first=i;}else if i>=self.first+capacity.max(1){self.first=i+1-capacity.max(1);}}}
    }}}
    fn restore(&mut self){self.hidden.fill(false);self.rows.clear();self.history.clear();self.first=0;}
}
struct RankGrid{width:usize,name:usize,cell:usize,metrics:Vec<usize>,first:usize,total:usize,capacity:usize}
impl RankGrid{
    fn new(width:usize,name:usize,cell:usize,columns:&mut RankColumns)->Self{
        let name=name.min(width.saturating_sub(cell+3));let capacity=(width.saturating_sub(name+2)/(cell+1)).max(1);
        let visible=columns.visible();columns.first=columns.first.min(visible.len().saturating_sub(capacity));
        let metrics:Vec<_>=visible.iter().skip(columns.first).take(capacity).copied().collect();
        Self{width:name+2+metrics.len()*(cell+1),name,cell,metrics,first:columns.first,total:visible.len(),capacity}
    }
    fn content_x(&self,col:usize)->usize{2+self.name+col*(self.cell+1)}
    fn metric_at(&self,x:usize,y:usize,count:usize)->Option<usize>{
        if y!=RANK_Y+1&&!(RANK_DATA..RANK_DATA+count).contains(&y){return None;}
        self.metrics.iter().enumerate().find_map(|(j,&m)|{let xx=self.content_x(j);(x>=xx&&x<xx+self.cell).then_some(m)})
    }
    fn layout_at(&self,x:usize,y:usize,count:usize)->Option<usize>{
        if x>=1&&x<1+self.name&&(RANK_DATA..RANK_DATA+count).contains(&y){Some(y-RANK_DATA)}else{None}
    }
}
#[derive(Clone,Copy)]
struct RankRange{lo:f64,hi:f64}
fn rank_ranges(rows:&[RankRow],hidden:&BTreeSet<PathBuf>)->[RankRange;RANK_COUNT]{
    let mut out=[RankRange{lo:f64::INFINITY,hi:f64::NEG_INFINITY};RANK_COUNT];
    for row in rows{if hidden.contains(&row.model.board.path){continue;}for(m,range)in out.iter_mut().enumerate(){
        if let Some(v)=rank_value(row,m){if v.is_finite(){range.lo=range.lo.min(v);range.hi=range.hi.max(v);}}
    }}out
}
fn rank_color(m:usize,v:f64,range:RankRange)->u8{
    if !v.is_finite()||!range.lo.is_finite()||!range.hi.is_finite(){return MUTED;}
    let span=range.hi-range.lo;if span.abs()<=1e-12*(1.0+range.lo.abs().max(range.hi.abs())){return gradient(0.5);}
    let t=((v-range.lo)/span).clamp(0.0,1.0);gradient(if rank_positive(m){t}else{1.0-t})
}
fn rank_order(rows:&[RankRow],filter:&str,metric:Option<usize>,ascending:bool,hidden:&BTreeSet<PathBuf>)->Vec<usize>{
    let filter=filter.to_ascii_lowercase();let mut order:Vec<_>=(0..rows.len()).filter(|&i|!hidden.contains(&rows[i].model.board.path)&&rows[i].model.board.name.to_ascii_lowercase().contains(&filter)).collect();
    order.sort_by(|&a,&b|{
        if let Some(m)=metric{let va=rank_value(&rows[a],m);let vb=rank_value(&rows[b],m);if va.is_none()!=vb.is_none(){return va.is_none().cmp(&vb.is_none());}}
        let cmp=if let Some(m)=metric{rank_value(&rows[a],m).unwrap_or(0.0).total_cmp(&rank_value(&rows[b],m).unwrap_or(0.0))}else{rows[a].model.board.name.cmp(&rows[b].model.board.name)};
        (if ascending{cmp}else{cmp.reverse()}).then_with(||rows[a].model.board.name.cmp(&rows[b].model.board.name))
    });order
}
fn rank_viewport(top:&mut usize,selected:&mut usize,total:usize,capacity:usize,follow:bool){
    let capacity=capacity.max(1);*selected=(*selected).min(total.saturating_sub(1));*top=(*top).min(total.saturating_sub(capacity));
    if follow{if *selected<*top{*top=*selected;}else if *selected>=top.saturating_add(capacity){*top=*selected+1-capacity;}}
    else if total>0{*selected=(*selected).clamp(*top,(top.saturating_add(capacity)-1).min(total-1));}
}
fn short_end(text:&str,width:usize)->String{let cs:Vec<_>=clean_text(text).chars().collect();if cs.len()<=width{cs.into_iter().collect()}else if width<=1{"…".into()}else{format!("…{}",cs[cs.len()-width+1..].iter().collect::<String>())}}
fn draw_rank_table(c:&mut Canvas,g:&RankGrid,rows:&[RankRow],order:&[usize],top:usize,selected:usize,shown:usize,metric:Option<usize>,ascending:bool,dp:usize,ranges:&[RankRange;RANK_COUNT])->usize{
    let bottom=RANK_DATA+shown.max(1);c.boxed(Rect{x:0,y:RANK_Y,w:g.width,h:bottom-RANK_Y+1},BORDER);
    c.line(0,RANK_Y+2,g.width,BORDER);c.put(0,RANK_Y+2,'├',BORDER);c.put(g.width-1,RANK_Y+2,'┤',BORDER);
    let arrow=if ascending{"↑"}else{"↓"};c.text(2,RANK_Y+1,&format!("Layout{}",if metric.is_none(){arrow}else{""}),if metric.is_none(){YELLOW}else{FG});
    c.hit(Rect{x:1,y:RANK_Y+1,w:g.name,h:1},Action::Command('n'));
    for(j,&m)in g.metrics.iter().enumerate(){let x=g.content_x(j);c.put(x-1,RANK_Y,'┬',BORDER);c.put(x-1,bottom,'┴',BORDER);
        for y in RANK_Y+1..bottom{c.put(x-1,y,'│',BORDER);}c.put(x-1,RANK_Y+2,'┼',BORDER);
        c.center(x,RANK_Y+1,g.cell,&format!("{}{}",rank_name(m),if metric==Some(m){arrow}else{""}),if metric==Some(m){YELLOW}else{FG});
        c.hit(Rect{x,y:RANK_Y+1,w:g.cell,h:1},Action::Metric(m));
    }
    let digits=order.len().to_string().len();
    for(offset,&i)in order.iter().skip(top).take(shown).enumerate(){let r=top+offset;let y=RANK_DATA+offset;let row=&rows[i];
        let prefix=format!("{}{:>digits$}. ",if r==selected{'›'}else{' '},r+1);let name=short_end(&row.model.board.name,g.name.saturating_sub(prefix.chars().count()));
        c.text(1,y,&format!("{prefix}{name}"),FG);c.hit(Rect{x:1,y,w:g.width-2,h:1},Action::Item(r));
        for(j,&m)in g.metrics.iter().enumerate(){let value=rank_value(row,m);let text=match value{Some(v)=>{let mut s=number(v,dp);if m==RANK_COVERAGE||m<N_METRICS&&!physical_metric(m){s.push('%');}s},None=>"n/a".into()};
            c.right(g.content_x(j)+1,y,g.cell-2,&text,value.map(|v|rank_color(m,v,ranges[m])).unwrap_or(MUTED));
        }
    }if order.is_empty(){c.text(2,RANK_DATA,"—",MUTED);}bottom
}
fn ranking(term:&mut Terminal,mut source:Source)->AppResult<()> {
    let(mut rows,mut errors)=load_ranking(&source)?;let mut metric=Some(RANK_SCORE);let mut ascending=true;let mut columns=RankColumns::new();
    let(mut selected,mut top)=(0,0);let mut filter=String::new();let mut status=String::new();
    loop{
        let order=rank_order(&rows,&filter,metric,ascending,&columns.rows);let ranges=rank_ranges(&rows,&columns.rows);
        let width=term.size.0.max(64);let height=term.size.1.max(12);let capacity=height.saturating_sub(RANK_DATA+3).max(1);
        rank_viewport(&mut top,&mut selected,order.len(),capacity,false);let shown=order.len().saturating_sub(top).min(capacity);
        let name_width=rows.iter().map(|r|r.model.board.name.chars().count()).max().unwrap_or(12).saturating_add(order.len().to_string().len()+3).clamp(18,30);
        let mut cw=10usize;for row in &rows{for m in 0..RANK_COUNT{if let Some(v)=rank_value(row,m){cw=cw.max(number(v,term.decimals()).chars().count()+3);}}}
        let grid=RankGrid::new(width,name_width,cw,&mut columns);let mut c=Canvas::ranking(width,height);
        header(&mut c,"Ranker",&source.name,"","c corpus | r reload | u restore | H all | q back");
        let bottom=draw_rank_table(&mut c,&grid,&rows,&order,top,selected,shown,metric,ascending,term.decimals(),&ranges);
        let mut footer=format!("{} layouts · travel u/100",order.len());let hidden=columns.hidden.iter().filter(|&&x|x).count();
        if hidden>0{footer.push_str(&format!(" · {hidden} columns hidden"));}if !columns.rows.is_empty(){footer.push_str(&format!(" · {} rows hidden",columns.rows.len()));}
        if grid.total>grid.metrics.len(){footer.push_str(&format!(" · columns {}–{}/{}",grid.first+1,grid.first+grid.metrics.len(),grid.total));}
        c.text(0,bottom+1,&short(&footer,width),MUTED);
        let selected_name=order.get(selected).map(|&i|rows[i].model.board.name.as_str()).unwrap_or("");
        c.text(0,bottom+2,&short(if status.is_empty(){selected_name}else{&status},width),CYAN);c.h=bottom+3;term.present(&c,0)?;
        let mut e=term.event()?;
        if let Event::Mouse{x,y,button:1,release:false,motion:false}=e {
            if let Some(offset)=grid.layout_at(x,y,shown){if let Some(&i)=order.get(top+offset){columns.hide_row(rows[i].model.board.path.clone());}status.clear();continue;}
            if let Some(m)=grid.metric_at(x,y,shown){columns.hide(m);status.clear();continue;}
        }
        let delta=match e{Event::Wheel(n)=>Some(n),Event::PageUp=>Some(-(capacity as i32)),Event::PageDown=>Some(capacity as i32),_=>None};
        if let Some(d)=delta{top=(top as i64+d as i64).max(0)as usize;rank_viewport(&mut top,&mut selected,order.len(),capacity,false);continue;}
        if let Some(a)=action_press(term,&c,&e,0){match a{
            Action::Metric(m)=>{if metric==Some(m){ascending=!ascending;}else{metric=Some(m);ascending=!rank_positive(m);}top=0;selected=0;continue;},
            Action::Item(r)=>{if selected==r{if let Some(&i)=order.get(r){inspect_row(term,&rows[i])?;}}selected=r;continue;},
            Action::Command(ch)=>e=Event::Char(ch),_=>{}
        }}
        match e{
            Event::Escape|Event::Quit|Event::Char('q')=>return Ok(()),Event::Enter=>if let Some(&i)=order.get(selected){inspect_row(term,&rows[i])?;},
            Event::Up|Event::Char('k')=>{selected=selected.saturating_sub(1);rank_viewport(&mut top,&mut selected,order.len(),capacity,true);},
            Event::Down|Event::Char('j')=>{selected=(selected+1).min(order.len().saturating_sub(1));rank_viewport(&mut top,&mut selected,order.len(),capacity,true);},
            Event::Left|Event::Char('h')=>columns.first=columns.first.saturating_sub(1),Event::Right|Event::Char('l')=>columns.first=columns.first.saturating_add(1),
            Event::Tab=>columns.first=columns.first.saturating_add(grid.capacity),Event::Home=>columns.first=0,Event::End=>columns.first=grid.total,
            Event::Char('u')=>columns.undo(grid.capacity),Event::Char('H')=>columns.restore(),Event::Char('.')=>term.precise=!term.precise,
            Event::Char('n')=>{if metric.is_none(){ascending=!ascending;}else{metric=None;ascending=true;}top=0;selected=0;},
            Event::Char('/')=>if let Some(q)=input_box(term,"", "",&filter)?{filter=q;top=0;selected=0;},
            Event::Char('e')=>info_page(term,"Errors",&errors)?,
            Event::Char('c')=>if let Some(path)=choose_path(term,CORPUS_DIR,"json","corpus-","")?{
                match load_source_tui(term,&path).and_then(|s|load_ranking(&s).map(|r|(s,r))){Ok((s,(r,e)))=>{source=s;rows=r;errors=e;status.clear();top=0;selected=0;},Err(e)=>status=e.to_string()}
            },
            Event::Char('r')=>{let result=(||->AppResult<(Source,Vec<RankRow>,Vec<String>)>{let path=corpus_by_name(&source.name).unwrap_or(source.path.clone());let s=load_source_tui(term,&path)?;let(r,e)=load_ranking(&s)?;Ok((s,r,e))})();
                match result{Ok((s,r,e))=>{source=s;rows=r;errors=e;status.clear();},Err(e)=>status=e.to_string()}
            },
            Event::Char('?')=>info_page(term,"Ranker",&[
                "Left-click header sorts. Middle-click a metric hides its column; middle-click a layout name hides its row.".into(),
                "u restores the last hidden item; H restores all. Left/right scroll columns; wheel scrolls layouts.".into(),
                "Colors use min/max of every non-hidden layout; green is preferable. Different key sets still require coverage checks.".into(),
                "SCORE uses saved detailed weights on this corpus. Candidate comparison uses its actual training objective.".into(),
                "Enter opens the selected layout; . changes precision; / filters names; e shows load errors.".into()
            ])?,_=>{}
        }
    }
}
fn candidate_view(term:&mut Terminal,p:&Problem,candidates:&[Candidate],selected:usize)->AppResult<usize>{
    if candidates.is_empty(){return Ok(selected);}
    let mut rows=Vec::new();for(i,item)in candidates.iter().enumerate(){let mut model=p.model.clone();model.original=item.arr.clone();model.board.name=format!("Candidate {:03}",i+1);model.board.path=PathBuf::from(format!("candidate-{i}"));
        let corpus=p.corpora[0].clone();let raw=full_raw(&item.arr,&corpus,&model.geometry);let metrics=metrics(&raw,&corpus);rows.push(RankRow{model,corpus,raw,metrics,score:item.score});}
    let mut columns=RankColumns::new();let wanted=[RANK_SCORE,SFB,SFS,TRAVEL,SFTRAVEL,LSB,DSB,CSB,SRAF,ROLL];for m in 0..RANK_COUNT{columns.hidden[m]=!wanted.contains(&m);}
    let mut chosen=selected.min(rows.len()-1);let(mut top,mut current)=(0,chosen);let mut metric=Some(RANK_SCORE);let mut ascending=true;
    loop{
        let order=rank_order(&rows,"",metric,ascending,&columns.rows);let ranges=rank_ranges(&rows,&columns.rows);let capacity=term.size.1.saturating_sub(RANK_DATA+3).max(1);
        rank_viewport(&mut top,&mut current,order.len(),capacity,true);let shown=order.len().saturating_sub(top).min(capacity);let g=RankGrid::new(term.size.0.max(64),18,10,&mut columns);
        let mut c=Canvas::ranking(term.size.0.max(64),term.size.1.max(12));header(&mut c,"Candidates",&p.corpora[0].name,&p.model.board.name,"q back");
        let bottom=draw_rank_table(&mut c,&g,&rows,&order,top,current,shown,metric,ascending,term.decimals(),&ranges);
        if let Some(&i)=order.get(current){let nearest=candidates.iter().enumerate().filter(|(j,_)|*j!=i).map(|(_,other)|letter_distance(&p.model,&other.arr,&candidates[i].arr)).min();
            if let Some(n)=nearest{c.text(0,bottom+1,&format!("Nearest candidate: {n} moved letters"),MUTED);}}
        c.h=bottom+2;term.present(&c,0)?;let e=term.event()?;
        if let Some(a)=action_press(term,&c,&e,0){match a{Action::Item(r)=>{if let Some(&i)=order.get(r){return Ok(i);}},Action::Metric(m)=>{if metric==Some(m){ascending=!ascending;}else{metric=Some(m);ascending=!rank_positive(m);}current=0;top=0;},_=>{}}}
        match e{Event::Enter=>{if let Some(&i)=order.get(current){chosen=i;}return Ok(chosen);},Event::Char('q')|Event::Escape|Event::Quit=>return Ok(chosen),
            Event::Up=>current=current.saturating_sub(1),Event::Down=>current=(current+1).min(order.len().saturating_sub(1)),
            Event::Left=>columns.first=columns.first.saturating_sub(1),Event::Right=>columns.first=columns.first.saturating_add(1),
            Event::Char('.')=>term.precise=!term.precise,_=>{}}
    }
}
