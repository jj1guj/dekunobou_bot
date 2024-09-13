use core::str;
use std::{collections::HashMap, sync::{atomic::{AtomicBool, AtomicU32}, Arc}};

use futures_util::{future::BoxFuture, stream::{SplitSink, SplitStream}, SinkExt, StreamExt, TryStreamExt};
use rand::SeedableRng;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use dekunobou;

#[derive(Serialize,Deserialize,Debug)]
struct WSResult{
	#[serde(rename = "type")]
	t:String,
	body:serde_json::Value,
}
#[derive(Serialize,Deserialize,Debug)]
struct WSChannel{
	#[serde(rename = "type")]
	t:String,
	id:String,
	body:serde_json::Value,
}
#[derive(Serialize,Deserialize,Debug)]
struct ReversiInvite{
	user:MiUser,
}
#[derive(Serialize,Deserialize,Debug)]
struct ConfigFile{
	instance:String,
	token:String,
	dekunobou:Option<String>,
	depth:u32,
	perfect_search_depth:u32,
}
#[derive(Serialize,Deserialize,Debug)]
struct MiUser{
	id:String,
	name:Option<String>,
	username:String,
	host:Option<String>,
	#[serde(rename = "isBot")]
	is_bot:bool,
	#[serde(rename = "isCat")]
	is_cat:bool,
}
#[derive(Serialize,Deserialize,Debug)]
struct MatchRequest{
	accept_only:bool,
	i:String,
	#[serde(rename = "userId")]
	user_id:String,
}
#[derive(Serialize,Deserialize,Debug)]
struct MatchResponse{
	id:String,
	#[serde(rename = "user1Id")]
	user1_id:String,
	#[serde(rename = "user2Id")]
	user2_id:String,
}
#[derive(Serialize,Deserialize,Debug)]
struct GameContext{
	id:String,
	user1_id:String,
	user2_id:String,
	user2_is_self:bool,
	user2_is_black:bool,
	user2_is_active_player:bool,
	board:DekunobouBoard,
	log:Vec<u8>,
}
impl GameContext{
	fn self_id(&self)->&str{
		if self.user2_is_self{
			self.user2_id.as_str()
		}else{
			self.user1_id.as_str()
		}
	}
	fn is_self_turn(&self)->bool{
		self.user2_is_active_player==self.user2_is_self
	}
	fn is_self_black(&self)->bool{
		self.user2_is_black==self.user2_is_self
	}
	async fn call_dekunobou_ffi(&mut self,config:&ConfigFile)->Option<u32>{
		let depth=config.depth;
		let perfect_search_depth=config.perfect_search_depth;
		let board_string = std::ffi::CString::new(self.board.0.as_str()).unwrap();
		let pos=unsafe { dekunobou::dekunobou(board_string.as_ptr(),!self.is_self_black(),depth,perfect_search_depth) };
		Some(pos)
	}
	async fn call_dekunobou_http(&mut self,client:&Client,config:&ConfigFile)->Option<u32>{
		println!("call_dekunobou");
		let mut req=DekunobouRequest{
			board:self.board.clone(),
			depth: 8,
			perfect_search_depth: 13,
			turn: if self.is_self_black(){
				0
			}else{
				1
			},
		};
		for _ in 0..3{
			let v={
				//dekunobou return int 32bit
				let res=client.put(config.dekunobou.as_ref()?.as_str()).header("Content-Type","application/json").body(serde_json::to_string(&req).unwrap()).send().await;
				let res=res.map_err(|e|eprintln!("{:?}",e)).ok()?;
				let res=res.bytes().await.map_err(|e|eprintln!("{:?}",e)).ok()?;
				serde_json::from_slice::<DekunobouResponse>(&res).map_err(|e|eprintln!("{:?}",e)).ok()
			};
			req.depth-=1;
			req.perfect_search_depth-=1;
			if let Some(v)=v{
				return u32::from_str_radix(&v.pos,10).ok();
			}
		}
		None
	}
	async fn put_stone_and_loop(&mut self,client:&Client,ws:&mut WSState,config:&ConfigFile){
		if !self.is_self_turn(){
			return;
		}
		loop{
			//自分の視点で置けるか確認する
			let list=MiBoard::from(self.board.clone()).legal_move_list(self.is_self_black());
			if list.is_empty(){
				//どこにも置けないなら自分の番を終了
				println!("どこにも置けないなら自分の番を終了");
				break;
			}
			self.put_stone(client, ws,config).await;
			//相手の視点で置けるか確認する
			let list=MiBoard::from(self.board.clone()).legal_move_list(!self.is_self_black());
			if !list.is_empty(){
				println!("どこかに置けるなら自分の番は終了{:?}",list);
				//どこかに置けるなら自分の番は終了
				break;
			}
		}
	}
	async fn put_stone(&mut self,client:&Client,ws:&mut WSState,config:&ConfigFile){
		if !self.is_self_turn(){
			return;
		}
		let mut map=serde_json::Map::new();
		let res=if config.dekunobou.is_some(){
			self.call_dekunobou_http(&client,config).await
		}else{
			self.call_dekunobou_ffi(&config).await
		};
		match res{
			Some(pos)=>{
				use rand::distributions::{Alphanumeric, DistString};
				let mut rng=rand::rngs::StdRng::from_entropy();
				let id = Alphanumeric.sample_string(&mut rng, 10).to_ascii_lowercase();
				map.insert("id".into(),id.into());
				if let Ok(pos)=u8::from_str_radix(pos.to_string().as_str(),10){
					self.board.put_stone(pos,self.is_self_black());
					self.board.debug_dump();
					self.log.push(pos);
					map.insert("pos".into(),serde_json::Value::Number(pos.into()));
					for _ in 0..2{
						let res=ws.send_channel("putStone".to_string(),Some(serde_json::Value::Object(map.clone()))).await;
						if let Err(e)=res{
							println!("putStone err{:}",e);
						}else{
							tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
							break;
						}
					}
				}
			},
			None=>{
				//投了する(未実装)
			}
		}
	}
}
#[derive(Serialize,Deserialize,Debug)]
struct ReversiStarted{
	black:u8,
}
#[derive(Serialize,Deserialize,Debug)]
struct MiBoard([[u8;8];8]);
impl MiBoard {
	fn put_stone(&mut self,pos:u8,is_black:bool){
		if pos>64{
			panic!();
		}
		let col=(pos%8) as usize;
		let row=(pos/8) as usize;

		self.0[row][col]=if is_black{
			1
		}else{
			2
		};
		let di=[0,0,-1,1,-1,1,-1,1];
		let dj=[-1,1,0,0,1,1,-1,-1];
		let (flip_limit,flip_count)=self.get_flip_limit(row,col,is_black);
		println!("flip_count {} {:?}",flip_count,flip_limit);
		for dir in 0..8{
			for i in 1..flip_limit[dir]{
				self.0[(row as isize+di[dir]*i as isize) as usize][(col as isize+dj[dir]*i as isize) as usize]=self.0[row][col];
			}
		}
	}
	fn legal_move_list(&self,is_black:bool)->Vec<u8>{
		let mut movelist=Vec::new();
		for i in 0..8{
			for j in 0..8{
				if self.0[i][j]==0{
					let (flip_limit,_)=self.get_flip_limit(i,j,is_black);
					for k in 0..8{
						if flip_limit[k]>1{
							movelist.push((8*i+j) as u8);
							break;
						}
					}
				}
			}
		}
		movelist
	}

	fn get_flip_limit(&self,row:usize,col:usize,is_black:bool)->([usize;8],usize){
		let self_color=if is_black{
			1
		}else{
			2
		};
		let enemy_color=if is_black{
			2
		}else{
			1
		};
		//返せる石の枚数が返り値
		let mut flip_count=0;
		//横左方向
		let mut flip_limit=[0usize;8];
		flip_limit[0]=0;
		for i in 1..(col+1){
			if self.0[row][col-i]!=enemy_color{
				if self.0[row][col-i]==self_color{
					flip_limit[0]=i;
				}
				break;
			}
		}
		if flip_limit[0]>1{
			flip_count+=flip_limit[0]-1;
		}

		//横右方向
		flip_limit[1]=0;
		for i in 1..8-col{
			if self.0[row][col+i]!=enemy_color{
				if self.0[row][col+i]==self_color{
					flip_limit[1]=i;
				}
				break;
			}
		}
		if flip_limit[1]>1{
			flip_count+=flip_limit[1]-1;
		}
		//縦上方向
		flip_limit[2]=0;
		for i in 1..(row+1){
			if self.0[row-i][col]!=enemy_color{
				if self.0[row-i][col]==self_color{
					flip_limit[2]=i;
				}
				break;
			}
		}
		if flip_limit[2]>1{
			flip_count+=flip_limit[2]-1;
		}
		//縦下方向
		flip_limit[3]=0;
		for i in 1..8-row{
			if self.0[row+i][col]!=enemy_color{
				if self.0[row+i][col]==self_color{
					flip_limit[3]=i;
				}
				break;
			}
		}
		if flip_limit[3]>1{
			flip_count+=flip_limit[3]-1;
		}
		//右斜め上方向
		flip_limit[4]=0;
		for i in 1..(row+1).min(8-col){
			if self.0[row-i][col+i]!=enemy_color{
				if self.0[row-i][col+i]==self_color{
					flip_limit[4]=i;
				}
				break;
			}
		}
		if flip_limit[4]>1{
			flip_count+=flip_limit[4]-1;
		}
		//右斜め下方向
		flip_limit[5]=0;
		for i in 1..(8-row).min(8-col){
			if self.0[row+i][col+i]!=enemy_color{
				if self.0[row+i][col+i]==self_color{
					flip_limit[5]=i;
				}
				break;
			}
		}
		if flip_limit[5]>1{
			flip_count+=flip_limit[5]-1;
		}
		//左斜め上方向
		flip_limit[6]=0;
		for i in 1..(row+1).min(col+1){
			if self.0[row-i][col-i]!=enemy_color{
				if self.0[row-i][col-i]==self_color{
					flip_limit[6]=i;
				}
				break;
			}
		}
		if flip_limit[6]>1{
			flip_count+=flip_limit[6]-1;
		}
		//左斜め下方向
		flip_limit[7]=0;
		for i in 1..(col+1).min(8-row){
			if self.0[row+i][col-i]!=enemy_color{
				if self.0[row+i][col-i]==self_color{
					flip_limit[7]=i;
				}
				break;
			}
		}
		if flip_limit[7]>1{
			flip_count+=flip_limit[7]-1;
		}
		(flip_limit,flip_count)
	}
}
impl From<DekunobouBoard> for MiBoard{
	fn from(value: DekunobouBoard) -> Self {
		let mut array=[[0u8;8];8];
		let mut x=0;
		let mut y=0;
		for c in value.0.chars(){
			if c=='1'{
				array[y][x]=1;
			}else if c=='2'{
				array[y][x]=2;
			}else{
				array[y][x]=0;
			}
			x+=1;
			if x==8{
				x=0;
				y+=1;
			}
		}
		Self(array)
	}
}
impl Into<DekunobouBoard> for MiBoard{
	fn into(self) -> DekunobouBoard {
		let mut s=String::new();
		for lines in self.0{
			for col in lines{
				match col{
					2=>s.push('2'),
					1=>s.push('1'),
					_=>s.push('0'),
				}
			}
		}
		DekunobouBoard(s)
	}
}
#[derive(Clone,Serialize,Deserialize,Debug)]
struct DekunobouBoard(String);
impl DekunobouBoard{
	fn new()->Self{
		//黒1/白2
		Self("0000000000000000000000000002100000012000000000000000000000000000".into())
	}
	fn debug_dump(&self){
		let mut x=0;
		let mut s=String::new();
		for c in self.0.chars(){
			x+=1;
			if c=='1'{
				s.push('@');
			}else if c=='2'{
				s.push('X');
			}else{
				s.push('_');
			}
			if x==8{
				x=0;
				s.push('\n');
			}
		}
		println!("{}",s);
	}
	fn put_stone(&mut self,pos:u8,is_black:bool){
		let mut mb:MiBoard=self.clone().into();
		mb.put_stone(pos,is_black);
		self.0=Into::<Self>::into(mb).0;
	}
	fn update_pos(&self,target:&DekunobouBoard)->u8{
		let mut index=0;
		let mut target=target.0.chars();
		for c in self.0.chars(){
			if let Some(target)=target.next(){
				if target!=c{
					break;
				}
			}
			index+=1;
		}
		index
	}
}
#[derive(Serialize,Deserialize,Debug)]
struct DekunobouRequest{
	board:DekunobouBoard,
	depth:u8,
	perfect_search_depth:u8,
	/**黒を置かせる時は0/白を置かせる時は1*/
	turn:u8,
}
#[derive(Serialize,Deserialize,Debug)]
struct DekunobouResponse{
	#[serde(rename = "move")]
	pos:String,
}

async fn check_invites(config:Arc<ConfigFile>,con:Arc<WSStream>,client:Client){
	let mut ws=WSState::new(con.clone()).await.unwrap();
	let (s,mut r)=tokio::sync::mpsc::channel(2);
	ws.open_channel(s, MiChannel::Reversi,None).await.unwrap();
	while let Some(event)=r.recv().await{
		match event.t.as_str(){
			"invited"=>{
				match serde_json::value::from_value::<ReversiInvite>(event.body){
					Ok(invite)=>{
						println!("invite from {:?}",&invite.user);
						let mut url=reqwest::Url::parse(config.instance.as_ref()).unwrap();
						url.set_path("api/reversi/match");
						let req=MatchRequest{
							accept_only:true,
							i:config.token.clone(),
							user_id:invite.user.id,
						};
						let res=client.post(url).header("Content-Type","application/json").body(serde_json::to_string(&req).unwrap()).send().await;
						if let Err(e)=res.as_ref(){
							eprintln!("{:?}",e);
						}
						if let Ok(v)=res.map(|v|v.bytes()){
							match v.await.map(|b|serde_json::from_slice::<MatchResponse>(&b).map_err(|e|(e,str::from_utf8(&b).map(|s|s.to_owned())))){
								Ok(Ok(res))=>{
									tokio::runtime::Handle::current().spawn(join_game(config.clone(),con.clone(),client.clone(),GameContext{
										id:res.id,
										user2_is_self:res.user1_id==req.user_id,//1が相手のIdなら,
										user2_is_black:false,
										user2_is_active_player:false,
										user1_id:res.user1_id,
										user2_id:res.user2_id,
										board:DekunobouBoard::new(),
										log:vec![],
									}));
								},
								e=>{
									eprintln!("{:?}",e);
								}
							}
						}
					},
					Err(e)=>{
						eprintln!("{:?}",e);
					}
				}
			},
			"error"=>{
				ws.close_channel().await;
				return;
			},
			_=>{
				println!("{:?}",event);
			},
		}
	}
}
async fn join_game(config:Arc<ConfigFile>,con:Arc<WSStream>,client:Client,mut game:GameContext){
	println!("join {}",game.id);
	let mut ws=WSState::new(con.clone()).await.unwrap();
	let (s,mut r)=tokio::sync::mpsc::channel(2);
	let mut parms=serde_json::Map::new();
	parms.insert("gameId".into(), game.id.as_str().into());
	ws.open_channel(s, MiChannel::ReversiGame,Some(serde_json::Value::Object(parms))).await.unwrap();
	while let Some(event)=r.recv().await{
		match event.t.as_str(){
			"updateSettings"=>{
				println!("updateSettings");
				println!("{:?}",event);
				if let Some(Some(key))=event.body.get("key").map(|k|k.as_str()){
					if key=="bw"{
						println!("bw {:?}",event.body.get("value"));
					}else{
						let _=ws.send_channel("cancel".to_string(),Some(serde_json::Value::Object(serde_json::Map::new()))).await;
					}
				}else{
					let _=ws.send_channel("cancel".to_string(),Some(serde_json::Value::Object(serde_json::Map::new()))).await;
				}
			}
			"changeReadyStates"=>{
				let key=if game.user2_is_self{
					"user2"
				}else{
					"user1"
				};
				if let Some(Some(key))=event.body.get(key).map(|k|k.as_bool()){
					if !key{
						let _=ws.send_channel("ready".to_string(),Some(serde_json::Value::Bool(true))).await;
					}
				}
			},
			"canceled"=>{
				println!("canceled");
				break;
			},
			"started"=>{
				println!("started");
				if let Some(Some(Some(black)))=event.body.get("game").map(|v|v.get("black").map(|k|k.as_u64())){
					game.user2_is_black=black==2;
					game.user2_is_active_player=game.user2_is_black;
				}
				//配置する位置を生成したり
				println!("{:?}",game);
				game.put_stone(&client,&mut ws,&config).await;
				game.board.debug_dump();
			},
			"log"=>{
				if let Some(Some(operation))=event.body.get("operation").map(|v|v.as_str()){
					if operation=="put"{
						if let Some(Some(pos))=event.body.get("pos").map(|v|v.as_u64()){
							let pos=pos as u8;
							//すでに配置済の場所には置けない
							if !game.log.contains(&pos){
								println!("log put {}",pos);
								game.board.put_stone(pos as u8,!game.is_self_black());
								game.log.push(pos);
								game.board.debug_dump();
								game.user2_is_active_player=game.user2_is_self;
								game.put_stone_and_loop(&client,&mut ws,&config).await;
								game.board.debug_dump();
							}
						}
					}else{
						println!("{:?}",event)
					}
				}
			}
			_=>{
				println!("{:?}",event)
			}
		}
	}
}
fn main() {
	let config=Arc::new(serde_json::from_reader(std::fs::File::open("config.json").unwrap()).unwrap());
	tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(async{
		let client=Client::default();
		let con=new_stream(&config,client.clone()).await.unwrap();
		check_invites(config,con,client).await;
		std::process::exit(1);
	});
}
async fn new_stream(config:&ConfigFile,client:Client)->Result<Arc<WSStream>,reqwest_websocket::Error>{
	use reqwest_websocket::RequestBuilderExt;
	let url=reqwest::Url::parse(config.instance.as_ref());
	let mut url=match url {
		Ok(url)=>url,
		Err(e)=>{
			eprintln!("{:?}",e);
			panic!("URL parse error");
		}
	};
	if url.scheme()=="http"{
		url.set_scheme("ws").unwrap();
	}else{
		url.set_scheme("wss").unwrap();
	}
	url.set_path("streaming");
	let query=format!("i={}",config.token.as_str());
	url.set_query(Some(&query));
	// create a GET request, upgrade it and send it.
	let response = client
		.get(url)
		.upgrade() // <-- prepares the websocket upgrade.
		.send()
		.await?;
	let websocket = response.into_websocket().await?;
	let ws=Arc::new(WSStream::new(websocket));
	let ws0=ws.clone();
	tokio::runtime::Handle::current().spawn(async move{
		let _=ws0.load().await;
	});
	println!("=============Open Connection===============");
	Ok(ws)
}
struct WSState{
	stream:Option<Arc<WSStream>>,
	now_stream:Option<u32>,
}
impl WSState{
	async fn new(stream:Arc<WSStream>)->Result<Self,reqwest_websocket::Error>{
		Ok(Self{
			stream:Some(stream),
			now_stream:None,
		})
	}
	async fn close_channel(&mut self){
		if let Some(id)=self.now_stream.take(){
			if let Err(e)=self.stream.as_ref().unwrap().close_channel(id).await{
				println!("close stream error {:?}",e);
			}
		}
		if let Some(stream)=self.stream.take(){
			drop(stream);
		//	stream.close_connection().await;
		}
	}
	async fn send_channel(&mut self,cmd_type:String,parms:Option<serde_json::Value>)->Result<(),reqwest_websocket::Error>{
		println!("=============Send Channel===============");
		let mut websocket=self.stream.as_ref().unwrap().send.lock().await;
		let mut map=serde_json::Map::new();
		map.insert("type".to_owned(), "ch".into());
		let mut body=serde_json::Map::new();
		body.insert("type".into(), cmd_type.into());
		body.insert("id".into(), self.now_stream.unwrap().to_string().into());
		if let Some(parms)=parms{
			body.insert("body".into(), parms);
		}
		map.insert("body".into(), body.into());
		let s=serde_json::to_string(&map).unwrap();
		println!("{}",s);
		websocket.send(reqwest_websocket::Message::Text(s.into())).await?;
		Ok(())
	}
	async fn open_channel(&mut self,sender:tokio::sync::mpsc::Sender<WSChannel>,ch:MiChannel,parms:Option<serde_json::Value>)->Result<(),reqwest_websocket::Error>{
		let sender=sender.clone();
		println!("=============Open Stream===============");
		let new_id=self.stream.as_ref().unwrap().open(move|res: WSChannel|{
			let sender=sender.clone();
			let f:BoxFuture<'static,()>=Box::pin(async move{
				if let Err(e)=sender.send(res).await{
					eprintln!("{:?}",e);
				}
			});
			f
		},ch,parms).await?;
		if let Some(id)=self.now_stream{
			let _=self.stream.as_ref().unwrap().close_channel(id).await;
		}
		self.now_stream=Some(new_id);
		Ok(())
	}
}
pub enum MiChannel{
	ReversiGame,
	Reversi,
}
impl MiChannel{
	pub fn id(&self)->&'static str{
		match self {
			MiChannel::ReversiGame => "reversiGame",
			MiChannel::Reversi => "reversi",
		}
	}
}
struct WSChannelListener(Box<dyn FnMut(WSChannel)->futures_util::future::BoxFuture<'static, ()>+Send+Sync>);
impl <F> From<F> for WSChannelListener where F:FnMut(WSChannel)->futures_util::future::BoxFuture<'static, ()>+Send+Sync+'static{
	fn from(value: F) -> Self {
		Self(Box::new(value))
	}
}
struct WSStream{
	channel_listener:Arc<Mutex<HashMap<u32,WSChannelListener>>>,
	last_id:AtomicU32,
	send: Arc<Mutex<SplitSink<reqwest_websocket::WebSocket, reqwest_websocket::Message>>>,
	recv: Mutex<Option<SplitStream<reqwest_websocket::WebSocket>>>,
	exit: Arc<AtomicBool>,
}
impl WSStream{
	fn new(websocket:reqwest_websocket::WebSocket)->Self{
		let (send,recv)=websocket.split();
		Self{
			channel_listener:Arc::new(Mutex::new(HashMap::new())),
			last_id:AtomicU32::new(0),
			send:Arc::new(Mutex::new(send)),
			recv:Mutex::new(Some(recv)),
			exit:Arc::new(AtomicBool::new(false)),
		}
	}
	async fn open(&self,listener:impl Into<WSChannelListener>,channel:MiChannel,parms:Option<serde_json::Value>)->Result<u32,reqwest_websocket::Error>{
		let mut websocket=self.send.lock().await;
		let id=self.last_id.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
		println!("open channel... {}",id);
		let mut channel_listener=self.channel_listener.lock().await;
		channel_listener.insert(id,listener.into());
		let mut map=serde_json::Map::new();
		map.insert("type".to_owned(), "connect".into());
		let mut body=serde_json::Map::new();
		body.insert("channel".into(), channel.id().into());
		body.insert("id".into(), id.to_string().into());
		if let Some(parms)=parms{
			body.insert("params".into(), parms);
		}
		map.insert("body".into(), body.into());
		websocket.send(reqwest_websocket::Message::Text(serde_json::to_string(&map).unwrap().into())).await?;
		println!("opend channel {}",id);
		Ok(id)
	}
	async fn close_channel(&self,id:u32)->Result<u32,reqwest_websocket::Error>{
		println!("close channel... {}",id);
		let mut websocket=self.send.lock().await;
		let q=format!("{{\"type\":\"disconnect\",\"body\":{{\"id\":\"{}\"}}}}",id);
		websocket.send(reqwest_websocket::Message::Text(q.into())).await?;
		let mut channel_listener=self.channel_listener.lock().await;
		channel_listener.remove(&id);
		println!("closed channel {}",id);
		Ok(id)
	}
	async fn load(&self){
		let websocket=self.recv.lock().await.take();
		if websocket.is_none(){
			return;
		}
		let mut websocket=websocket.unwrap();
		let channel_listener=self.channel_listener.clone();
		let channel_listener0=self.channel_listener.clone();
		let sender=self.send.clone();
		let exit0=self.exit.clone();
		std::thread::spawn(move||{
			let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
			let handle=rt.spawn(async move{
				while let Ok(Some(message)) = websocket.try_next().await {
					match message {
						reqwest_websocket::Message::Text(text) =>{
							if let Ok(Some(channel))=serde_json::from_str::<WSResult>(text.as_str()).map(|res|{
								if res.t.as_str()=="channel"{
									serde_json::value::from_value::<WSChannel>(res.body).ok()
								}else{
									None
								}
							}){
								if let Ok(id)=u32::from_str_radix(channel.id.as_str(),10){
									let mut r=channel_listener.lock().await;
									if let Some(handle)=r.get_mut(&id){
										handle.0(channel).await;
									}else{
										println!("unknown channel event {}",id);
									}
								}
							}else{
								println!("parse error {}",text);
							}
						},
						_=>{}
					}
				}
				println!("close websocket");
			});
			rt.block_on(async{
				while !exit0.load(std::sync::atomic::Ordering::Relaxed){
					let mut websocket=sender.lock().await;
					if let Err(e)=websocket.send(reqwest_websocket::Message::Text("h".into())).await{
						println!("ping error {:?}",e);
						let mut r=channel_listener0.lock().await;
						for (id,handle) in r.iter_mut(){
							handle.0(WSChannel { t: "error".to_owned(), id: id.to_string(), body: serde_json::Value::Null }).await;
						}
					}else{
						println!("ping ok");
					}
					drop(websocket);
					tokio::time::sleep(tokio::time::Duration::from_millis(60*1000)).await;
				}
			});
			handle.abort();
		});
	}
	async fn close_connection(&self){
		println!("close connection...");
		self.exit.store(true,std::sync::atomic::Ordering::Relaxed);
		let mut websocket=self.send.lock().await;
		let res=websocket.close().await;
		println!("closed connection {:?}",res);
	}
}
