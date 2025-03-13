use syn::visit_mut::VisitMut;
use xx_macros::*;

struct HasAwait(bool);

impl VisitMut for HasAwait {
	fn visit_item_mut(&mut self, _: &mut Item) {}

	fn visit_expr_async_mut(&mut self, _: &mut ExprAsync) {}

	fn visit_expr_closure_mut(&mut self, _: &mut ExprClosure) {}

	fn visit_expr_await_mut(&mut self, _: &mut ExprAwait) {
		self.0 = true;
	}

	fn visit_macro_mut(&mut self, mac: &mut Macro) {
		visit_macro_body(self, mac);
	}
}

declare_attribute_macro! {
	/// Declare a main function with xx-pulse
	///
	/// # Example
	///
	/// ```
	/// use xx_pulse::main;
	///
	/// #[main]
	/// async fn main() {
	/// 	do_async_stuff().await;
	/// }
	/// ```
	pub fn main(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
		let mut func = parse2::<ItemFn>(item)?;

		attr.require_empty()?;

		func.sig.asyncness.take();
		func.attrs.push(parse_quote! {
			#[::xx_pulse::asynchronous(sync)]
		});

		let pos = func.block.stmts.iter_mut().position(|stmt| {
			let mut has_await = HasAwait(false);

			has_await.visit_stmt_mut(stmt);
			has_await.0
		});

		if let Some(pos) = pos {
			let sync: Vec<_> = func.block.stmts.drain(0..pos).collect();
			let block = &func.block;

			func.block = parse_quote! {{
				#(#sync)*

				::xx_pulse::Runtime::new()
					.expect("Failed to start runtime")
					.block_on(async #block)
			}};
		}

		Ok(func.to_token_stream())
	}
}
