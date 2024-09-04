use std::ffi::c_char;

#[link(name = "dekunobou")]
extern "C"  {
	pub fn dekunobou(board_string:*const c_char, turn:bool, depth:u32, perfect_search_depth:u32)->u32;
}
