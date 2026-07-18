use crate::frontend::ast::{
    BinaryOp, DictEntry, EnumDef, EnumTupleFieldDef, EnumVariantDef, EnumVariantPayloadDef, Expr,
    Function, FunctionParam, InherentImplDef, Item, MatchArm, MatchNamedFieldPattern, MatchPattern,
    ModuleDecl, ParForReduction, Program, ReductionOp, Span, Stmt, StructDef, StructField,
    StructInitField, TraitDef, TraitImplDef, TraitMethodSig, TypeName, UnaryOp, UseDecl,
};
use crate::frontend::diagnostics::Diagnostic;
use crate::frontend::lexer::{Token, TokenKind};

pub fn parse(tokens: &[Token]) -> Result<Program, Diagnostic> {
    let mut parser = Parser::new(tokens);
    parser.parse_program().map_err(|diagnostic| {
        diagnostic.with_source(tokens.first().map_or(0, |token| token.source_id))
    })
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self { tokens, pos: 0 }
    }

    fn parse_program(&mut self) -> Result<Program, Diagnostic> {
        let mut items = Vec::new();
        while !self.matches(TokenKind::Eof) {
            let public = if self.matches(TokenKind::Pub) {
                self.advance();
                true
            } else {
                false
            };
            if self.matches(TokenKind::Struct) {
                items.push(Item::Struct(self.parse_struct(public)?));
            } else if self.matches(TokenKind::Enum) {
                items.push(Item::Enum(self.parse_enum(public)?));
            } else if self.matches(TokenKind::Trait) {
                items.push(Item::Trait(self.parse_trait(public)?));
            } else if self.matches(TokenKind::Impl) {
                if public {
                    let token = self.peek();
                    return Err(Diagnostic::new(
                        "'pub' is not allowed on impl blocks; visibility belongs to the type and trait",
                        token.line,
                        token.column,
                    ));
                }
                items.push(self.parse_impl()?);
            } else if self.matches(TokenKind::Fn) {
                items.push(Item::Function(self.parse_function(public)?));
            } else if self.matches(TokenKind::Mod) {
                items.push(Item::Module(self.parse_module_decl(public)?));
            } else if self.matches(TokenKind::Use) {
                items.push(Item::Use(self.parse_use_decl(public)?));
            } else {
                let token = self.peek();
                return Err(Diagnostic::new(
                    "expected 'struct', 'enum', 'trait', 'impl', 'fn', 'mod', 'use', or 'pub'",
                    token.line,
                    token.column,
                ));
            }
        }
        Ok(Program { items })
    }

    fn parse_module_decl(&mut self, public: bool) -> Result<ModuleDecl, Diagnostic> {
        let span = self.advance_with_span();
        let name = self.expect_ident("expected module name after 'mod'")?;
        self.expect(
            TokenKind::Semicolon,
            "expected ';' after module declaration",
        )?;
        Ok(ModuleDecl { name, public, span })
    }

    fn parse_use_decl(&mut self, public: bool) -> Result<UseDecl, Diagnostic> {
        let span = self.advance_with_span();
        let module = self.expect_ident("expected module name after 'use'")?;
        self.expect(TokenKind::ColonColon, "expected '::' in use declaration")?;
        let name = self.expect_ident("expected imported item name")?;
        let alias = if self.matches(TokenKind::As) {
            self.advance();
            Some(self.expect_ident("expected import alias after 'as'")?)
        } else {
            None
        };
        self.expect(TokenKind::Semicolon, "expected ';' after use declaration")?;
        Ok(UseDecl {
            module,
            name,
            alias,
            public,
            span,
        })
    }

    fn parse_enum(&mut self, public: bool) -> Result<EnumDef, Diagnostic> {
        let start = self.advance_with_span();
        let name = self.expect_ident("expected enum name")?;
        let type_params = self.parse_type_parameter_names()?;
        self.expect(TokenKind::LBrace, "expected '{' after enum name")?;
        let mut variants = Vec::new();
        while !self.matches(TokenKind::RBrace) {
            let variant_span = self.peek_span();
            let variant_name = self.expect_ident("expected enum variant name")?;
            let payload = if self.matches(TokenKind::LParen) {
                self.advance();
                let mut fields = Vec::new();
                if !self.matches(TokenKind::RParen) {
                    loop {
                        let field_span = self.peek_span();
                        fields.push(EnumTupleFieldDef {
                            ty: self.parse_type()?,
                            span: field_span,
                        });
                        if self.matches(TokenKind::Comma) {
                            self.advance();
                            if self.matches(TokenKind::RParen) {
                                break;
                            }
                            continue;
                        }
                        break;
                    }
                }
                self.expect(TokenKind::RParen, "expected ')' after enum tuple payload")?;
                EnumVariantPayloadDef::Tuple(fields)
            } else if self.matches(TokenKind::LBrace) {
                self.advance();
                let mut fields = Vec::new();
                while !self.matches(TokenKind::RBrace) {
                    let field_span = self.peek_span();
                    let field_name = self.expect_ident("expected enum payload field name")?;
                    self.expect(TokenKind::Colon, "expected ':' after enum payload field")?;
                    fields.push(StructField {
                        name: field_name,
                        ty: self.parse_type()?,
                        embedded: false,
                        span: field_span,
                    });
                    self.expect_struct_member_sep(
                        "expected ';' or ',' after enum payload field type",
                    )?;
                }
                self.expect(TokenKind::RBrace, "expected '}' after named enum payload")?;
                EnumVariantPayloadDef::Named(fields)
            } else {
                EnumVariantPayloadDef::Unit
            };
            variants.push(EnumVariantDef {
                name: variant_name,
                payload,
                span: variant_span,
            });
            if self.matches(TokenKind::Comma) {
                self.advance();
                if self.matches(TokenKind::RBrace) {
                    break;
                }
            } else if !self.matches(TokenKind::RBrace) {
                let token = self.peek();
                return Err(Diagnostic::new(
                    "expected ',' between enum variants",
                    token.line,
                    token.column,
                ));
            }
        }
        self.expect(TokenKind::RBrace, "expected '}' after enum")?;
        Ok(EnumDef {
            name,
            public,
            type_params,
            variants,
            span: start,
        })
    }

    fn parse_type_parameter_names(&mut self) -> Result<Vec<String>, Diagnostic> {
        if !self.matches(TokenKind::Less) {
            return Ok(Vec::new());
        }
        self.advance();
        let mut params = Vec::new();
        if self.matches(TokenKind::Greater) {
            let token = self.peek();
            return Err(Diagnostic::new(
                "generic declaration requires at least one type parameter",
                token.line,
                token.column,
            ));
        }
        loop {
            params.push(self.expect_ident("expected generic type parameter")?);
            if self.matches(TokenKind::Comma) {
                self.advance();
                continue;
            }
            break;
        }
        self.expect(
            TokenKind::Greater,
            "expected '>' after generic type parameters",
        )?;
        Ok(params)
    }

    fn parse_struct(&mut self, public: bool) -> Result<StructDef, Diagnostic> {
        let start = self.advance_with_span();
        let name = self.expect_ident("expected struct name")?;
        self.expect(TokenKind::LBrace, "expected '{' after struct name")?;
        let mut fields = Vec::new();
        let mut embed_index = 0usize;
        while !self.matches(TokenKind::RBrace) {
            let field_span = self.peek_span();
            if self.matches(TokenKind::Embed) {
                self.advance();
                let field_type = self.parse_type()?;
                fields.push(StructField {
                    name: format!("__embed_{}", embed_index),
                    ty: field_type,
                    embedded: true,
                    span: field_span,
                });
                embed_index += 1;
                self.expect_struct_member_sep("expected ';' or ',' after embedded field")?;
                continue;
            }
            let field_name = self.expect_ident("expected field name")?;
            self.expect(TokenKind::Colon, "expected ':' after field name")?;
            let field_type = self.parse_type()?;
            fields.push(StructField {
                name: field_name,
                ty: field_type,
                embedded: false,
                span: field_span,
            });
            self.expect_struct_member_sep("expected ';' or ',' after field type")?;
        }
        self.expect(TokenKind::RBrace, "expected '}' after struct")?;
        Ok(StructDef {
            name,
            public,
            fields,
            span: start,
        })
    }

    fn parse_trait(&mut self, public: bool) -> Result<TraitDef, Diagnostic> {
        let start = self.advance_with_span();
        let name = self.expect_ident("expected trait name")?;
        self.expect(TokenKind::LBrace, "expected '{' after trait name")?;
        let mut methods = Vec::new();
        while !self.matches(TokenKind::RBrace) {
            let span = self.peek_span();
            self.expect(TokenKind::Fn, "expected 'fn' in trait body")?;
            let method_name = self.expect_ident("expected trait method name")?;
            self.expect(TokenKind::LParen, "expected '('")?;
            let params = self.parse_function_params()?;
            self.expect(TokenKind::RParen, "expected ')'")?;
            let return_type = if self.matches(TokenKind::Minus) {
                self.advance();
                self.expect(TokenKind::Greater, "expected '>' in return type")?;
                Some(self.parse_type()?)
            } else {
                None
            };
            self.expect(
                TokenKind::Semicolon,
                "expected ';' after trait method signature",
            )?;
            methods.push(TraitMethodSig {
                name: method_name,
                params,
                return_type,
                span,
            });
        }
        self.expect(TokenKind::RBrace, "expected '}' after trait body")?;
        Ok(TraitDef {
            name,
            public,
            methods,
            span: start,
        })
    }

    fn parse_impl(&mut self) -> Result<Item, Diagnostic> {
        let start = self.advance_with_span();
        let name = self.expect_ident("expected trait or type name after impl")?;
        let (trait_name, for_type) = if self.matches(TokenKind::For) {
            self.advance();
            let for_type = self.expect_ident("expected target type in trait impl")?;
            (Some(name), for_type)
        } else {
            (None, name)
        };
        self.expect(TokenKind::LBrace, "expected '{' after impl header")?;
        let mut methods = Vec::new();
        while !self.matches(TokenKind::RBrace) {
            methods.push(self.parse_function(false)?);
        }
        self.expect(TokenKind::RBrace, "expected '}' after impl body")?;
        if let Some(trait_name) = trait_name {
            Ok(Item::Impl(TraitImplDef {
                trait_name,
                for_type,
                methods,
                span: start,
            }))
        } else {
            Ok(Item::InherentImpl(InherentImplDef {
                for_type,
                methods,
                span: start,
            }))
        }
    }

    fn parse_function(&mut self, public: bool) -> Result<Function, Diagnostic> {
        let start = self.advance_with_span();
        let name = self.expect_ident("expected function name")?;
        self.expect(TokenKind::LParen, "expected '('")?;
        let params = self.parse_function_params()?;
        self.expect(TokenKind::RParen, "expected ')'")?;
        let return_type = if self.matches(TokenKind::Minus) {
            self.advance();
            self.expect(TokenKind::Greater, "expected '>' in return type")?;
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(TokenKind::LBrace, "expected '{'")?;

        let mut stmts = Vec::new();
        while !self.matches(TokenKind::RBrace) {
            stmts.push(self.parse_stmt()?);
        }
        self.expect(TokenKind::RBrace, "expected '}'")?;
        Ok(Function {
            name,
            public,
            params,
            return_type,
            body: stmts,
            span: start,
        })
    }

    fn parse_function_params(&mut self) -> Result<Vec<FunctionParam>, Diagnostic> {
        let mut params = Vec::new();
        if !self.matches(TokenKind::RParen) {
            loop {
                let param_span = self.peek_span();
                let param_name = self.expect_ident("expected parameter name")?;
                self.expect(TokenKind::Colon, "expected ':' after parameter name")?;
                let param_ty = self.parse_type()?;
                params.push(FunctionParam {
                    name: param_name,
                    ty: param_ty,
                    span: param_span,
                });
                if self.matches(TokenKind::Comma) {
                    self.advance();
                    if self.matches(TokenKind::RParen) {
                        break;
                    }
                    continue;
                }
                break;
            }
        }
        Ok(params)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, Diagnostic> {
        if self.matches(TokenKind::LBrace) {
            return self.parse_block();
        }

        if self.matches(TokenKind::If) {
            return self.parse_if();
        }

        if self.matches(TokenKind::While) {
            return self.parse_while();
        }

        if self.matches(TokenKind::Loop) {
            return self.parse_loop();
        }

        if self.matches(TokenKind::For) {
            return self.parse_for();
        }

        if self.matches(TokenKind::ParFor) {
            return self.parse_parfor();
        }

        if self.matches(TokenKind::Foreach) {
            return self.parse_foreach();
        }

        if self.matches(TokenKind::Break) {
            let span = self.advance_with_span();
            self.expect(TokenKind::Semicolon, "expected ';' after break")?;
            return Ok(Stmt::Break { span });
        }

        if self.matches(TokenKind::Continue) {
            let span = self.advance_with_span();
            self.expect(TokenKind::Semicolon, "expected ';' after continue")?;
            return Ok(Stmt::Continue { span });
        }

        if self.matches(TokenKind::Return) {
            let span = self.advance_with_span();
            let expr = if self.matches(TokenKind::Semicolon) {
                None
            } else {
                Some(self.parse_expr()?)
            };
            self.expect(TokenKind::Semicolon, "expected ';' after return")?;
            return Ok(Stmt::Return { expr, span });
        }

        if self.matches(TokenKind::Let) {
            let start = self.advance_with_span();
            let mut mutable = false;
            if self.matches(TokenKind::Mut) {
                self.advance();
                mutable = true;
            }
            let name = self.expect_ident("expected identifier after let")?;
            let ty = if self.matches(TokenKind::Colon) {
                self.advance();
                Some(self.parse_type()?)
            } else {
                None
            };
            self.expect(TokenKind::Equal, "expected '=' after identifier")?;
            let expr = self.parse_expr()?;
            self.expect(TokenKind::Semicolon, "expected ';' after let")?;
            return Ok(Stmt::Let {
                name,
                mutable,
                ty,
                expr,
                span: start,
            });
        }

        if self.matches(TokenKind::Print) {
            let start = self.advance_with_span();
            self.expect(TokenKind::LParen, "expected '(' after print")?;
            let expr = self.parse_expr()?;
            self.expect(TokenKind::RParen, "expected ')' after print")?;
            self.expect(TokenKind::Semicolon, "expected ';' after print")?;
            return Ok(Stmt::Print { expr, span: start });
        }

        if self.matches(TokenKind::Exit) {
            let start = self.advance_with_span();
            self.expect(TokenKind::LParen, "expected '(' after exit")?;
            let expr = self.parse_expr()?;
            self.expect(TokenKind::RParen, "expected ')' after exit")?;
            self.expect(TokenKind::Semicolon, "expected ';' after exit")?;
            return Ok(Stmt::Exit { expr, span: start });
        }

        if self.matches(TokenKind::BenchLoop) {
            let start = self.advance_with_span();
            self.expect(TokenKind::LParen, "expected '(' after benchloop")?;
            let iterations = self.parse_expr()?;
            self.expect(TokenKind::RParen, "expected ')' after benchloop")?;
            self.expect(TokenKind::Semicolon, "expected ';' after benchloop")?;
            return Ok(Stmt::BenchLoop {
                iterations,
                span: start,
            });
        }

        if self.matches(TokenKind::Assert) {
            return self.parse_assert();
        }

        if self.matches(TokenKind::Panic) {
            return self.parse_panic();
        }

        if self.matches_ident() {
            let span = self.peek_span();
            let name = self.expect_ident("expected identifier")?;
            if self.matches(TokenKind::ColonColon) {
                self.advance();
                let member = self.expect_ident("expected associated function after '::'")?;
                self.expect(TokenKind::LParen, "expected '(' after associated function")?;
                let mut args = Vec::new();
                if !self.matches(TokenKind::RParen) {
                    loop {
                        args.push(self.parse_expr()?);
                        if self.matches(TokenKind::Comma) {
                            self.advance();
                            if self.matches(TokenKind::RParen) {
                                break;
                            }
                            continue;
                        }
                        break;
                    }
                }
                self.expect(TokenKind::RParen, "expected ')' after associated call")?;
                self.expect(TokenKind::Semicolon, "expected ';' after associated call")?;
                return Ok(Stmt::Call {
                    name: format!("{name}__{member}"),
                    args,
                    span,
                });
            }
            if self.matches(TokenKind::LBracket) {
                self.advance();
                let index = self.parse_expr()?;
                self.expect(TokenKind::RBracket, "expected ']' after index")?;
                self.expect(TokenKind::Equal, "expected '=' after indexed target")?;
                let expr = self.parse_expr()?;
                self.expect(TokenKind::Semicolon, "expected ';' after assignment")?;
                return Ok(Stmt::AssignIndex {
                    name,
                    index,
                    expr,
                    span,
                });
            }
            if self.matches(TokenKind::Dot) {
                self.advance();
                let member = self.expect_ident("expected field or method name after '.'")?;
                if self.matches(TokenKind::LBracket) {
                    self.advance();
                    let index = self.parse_expr()?;
                    self.expect(TokenKind::RBracket, "expected ']' after index")?;
                    self.expect(TokenKind::Equal, "expected '=' after indexed target")?;
                    let expr = self.parse_expr()?;
                    self.expect(TokenKind::Semicolon, "expected ';' after assignment")?;
                    return Ok(Stmt::AssignStructListIndex {
                        receiver: name,
                        field: member,
                        index,
                        expr,
                        span,
                    });
                }
                if self.matches(TokenKind::Equal) {
                    self.advance();
                    let expr = self.parse_expr()?;
                    self.expect(TokenKind::Semicolon, "expected ';' after assignment")?;
                    return Ok(Stmt::AssignField {
                        receiver: name,
                        field: member,
                        expr,
                        span,
                    });
                }
                if self.matches(TokenKind::Dot) {
                    self.advance();
                    let method =
                        self.expect_ident("expected method name after struct field '.'")?;
                    self.expect(TokenKind::LParen, "expected '(' after method name")?;
                    let mut args = Vec::new();
                    if !self.matches(TokenKind::RParen) {
                        loop {
                            args.push(self.parse_expr()?);
                            if self.matches(TokenKind::Comma) {
                                self.advance();
                                if self.matches(TokenKind::RParen) {
                                    break;
                                }
                                continue;
                            }
                            break;
                        }
                    }
                    self.expect(TokenKind::RParen, "expected ')' after method call")?;
                    self.expect(TokenKind::Semicolon, "expected ';' after method call")?;
                    return Ok(Stmt::StructListMethodCall {
                        receiver: name,
                        field: member,
                        name: method,
                        args,
                        span,
                    });
                }
                self.expect(TokenKind::LParen, "expected '(' after method name")?;
                let mut args = Vec::new();
                if !self.matches(TokenKind::RParen) {
                    loop {
                        args.push(self.parse_expr()?);
                        if self.matches(TokenKind::Comma) {
                            self.advance();
                            if self.matches(TokenKind::RParen) {
                                break;
                            }
                            continue;
                        }
                        break;
                    }
                }
                self.expect(TokenKind::RParen, "expected ')' after method call")?;
                self.expect(TokenKind::Semicolon, "expected ';' after method call")?;
                return Ok(Stmt::MethodCall {
                    receiver: name,
                    name: member,
                    args,
                    span,
                });
            }
            if self.matches(TokenKind::Equal) {
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(TokenKind::Semicolon, "expected ';' after assignment")?;
                return Ok(Stmt::Assign { name, expr, span });
            }
            if self.matches(TokenKind::LParen) {
                self.advance();
                let mut args = Vec::new();
                if !self.matches(TokenKind::RParen) {
                    loop {
                        args.push(self.parse_expr()?);
                        if self.matches(TokenKind::Comma) {
                            self.advance();
                            if self.matches(TokenKind::RParen) {
                                break;
                            }
                            continue;
                        }
                        break;
                    }
                }
                self.expect(TokenKind::RParen, "expected ')' after call")?;
                self.expect(TokenKind::Semicolon, "expected ';' after call")?;
                return Ok(Stmt::Call { name, args, span });
            }
            return Err(Diagnostic::new(
                "expected assignment or call",
                span.line,
                span.column,
            ));
        }

        let token = self.peek();
        Err(Diagnostic::new(
            "expected statement",
            token.line,
            token.column,
        ))
    }

    fn parse_block(&mut self) -> Result<Stmt, Diagnostic> {
        let start = self.advance_with_span();
        let mut stmts = Vec::new();
        while !self.matches(TokenKind::RBrace) {
            if self.matches(TokenKind::Eof) {
                let token = self.peek();
                return Err(Diagnostic::new(
                    "unterminated block",
                    token.line,
                    token.column,
                ));
            }
            stmts.push(self.parse_stmt()?);
        }
        self.expect(TokenKind::RBrace, "expected '}' after block")?;
        Ok(Stmt::Block { stmts, span: start })
    }

    fn parse_if(&mut self) -> Result<Stmt, Diagnostic> {
        let start = self.advance_with_span();
        let cond = self.parse_expr()?;
        self.expect(TokenKind::LBrace, "expected '{' after if condition")?;
        let mut then_branch = Vec::new();
        while !self.matches(TokenKind::RBrace) {
            then_branch.push(self.parse_stmt()?);
        }
        self.expect(TokenKind::RBrace, "expected '}' after if block")?;

        let else_branch = if self.matches(TokenKind::Else) {
            self.advance();
            if self.matches(TokenKind::If) {
                let nested_if = self.parse_if()?;
                Some(vec![nested_if])
            } else {
                self.expect(TokenKind::LBrace, "expected '{' after else")?;
                let mut else_branch = Vec::new();
                while !self.matches(TokenKind::RBrace) {
                    else_branch.push(self.parse_stmt()?);
                }
                self.expect(TokenKind::RBrace, "expected '}' after else block")?;
                Some(else_branch)
            }
        } else {
            None
        };

        Ok(Stmt::If {
            cond,
            then_branch,
            else_branch,
            span: start,
        })
    }

    fn parse_while(&mut self) -> Result<Stmt, Diagnostic> {
        let start = self.advance_with_span();
        let cond = self.parse_expr()?;
        self.expect(TokenKind::LBrace, "expected '{' after while condition")?;
        let mut body = Vec::new();
        while !self.matches(TokenKind::RBrace) {
            body.push(self.parse_stmt()?);
        }
        self.expect(TokenKind::RBrace, "expected '}' after while body")?;
        Ok(Stmt::While {
            cond,
            body,
            span: start,
        })
    }

    fn parse_loop(&mut self) -> Result<Stmt, Diagnostic> {
        let start = self.advance_with_span();
        self.expect(TokenKind::LBrace, "expected '{' after loop")?;
        let mut body = Vec::new();
        while !self.matches(TokenKind::RBrace) {
            body.push(self.parse_stmt()?);
        }
        self.expect(TokenKind::RBrace, "expected '}' after loop body")?;
        Ok(Stmt::Loop { body, span: start })
    }

    fn parse_for(&mut self) -> Result<Stmt, Diagnostic> {
        let start = self.advance_with_span();
        let name = self.expect_ident("expected loop variable name after for")?;
        self.expect(TokenKind::In, "expected 'in' after loop variable")?;
        let start_expr = self.parse_expr()?;
        self.expect(TokenKind::DotDot, "expected '..' in for range")?;
        let end_expr = self.parse_expr()?;
        self.expect(TokenKind::LBrace, "expected '{' after for range")?;
        let mut body = Vec::new();
        while !self.matches(TokenKind::RBrace) {
            body.push(self.parse_stmt()?);
        }
        self.expect(TokenKind::RBrace, "expected '}' after for body")?;
        Ok(Stmt::For {
            name,
            start: start_expr,
            end: end_expr,
            body,
            span: start,
        })
    }

    fn parse_parfor(&mut self) -> Result<Stmt, Diagnostic> {
        let start = self.advance_with_span();
        let name = self.expect_ident("expected loop variable name after parfor")?;
        self.expect(TokenKind::In, "expected 'in' after parfor variable")?;
        let start_expr = self.parse_expr()?;
        self.expect(TokenKind::DotDot, "expected '..' in parfor range")?;
        let end_expr = self.parse_expr()?;
        let mut body = Vec::new();
        let mut reduction = None;

        if self.matches_ident_text("reduce") {
            let reduction_span = self.peek_span();
            self.advance();
            let op_name = self.expect_ident("expected reduction op: sum|min|max")?;
            let op = match op_name.as_str() {
                "sum" => ReductionOp::Sum,
                "min" => ReductionOp::Min,
                "max" => ReductionOp::Max,
                _ => {
                    let token = self.peek();
                    return Err(Diagnostic::new(
                        "unknown reduction op (expected sum|min|max)",
                        token.line,
                        token.column,
                    ));
                }
            };
            self.expect_ident_text("into", "expected 'into' after reduction op")?;
            let target = self.expect_ident("expected reduction target variable")?;
            self.expect(
                TokenKind::LBrace,
                "expected '{' before reduction expression",
            )?;
            let expr = self.parse_expr()?;
            if self.matches(TokenKind::Semicolon) {
                self.advance();
            }
            self.expect(TokenKind::RBrace, "expected '}' after reduction expression")?;
            reduction = Some(ParForReduction {
                op,
                target,
                expr,
                span: reduction_span,
            });
        } else {
            self.expect(TokenKind::LBrace, "expected '{' after parfor range")?;
            while !self.matches(TokenKind::RBrace) {
                body.push(self.parse_stmt()?);
            }
            self.expect(TokenKind::RBrace, "expected '}' after parfor body")?;
        }

        let stmt = Stmt::ParFor {
            name,
            start: start_expr,
            end: end_expr,
            body,
            reduction,
            span: start,
        };
        if self.matches(TokenKind::Semicolon) {
            self.advance();
        }
        Ok(stmt)
    }

    fn parse_foreach(&mut self) -> Result<Stmt, Diagnostic> {
        let start = self.advance_with_span();
        let name = self.expect_ident("expected loop variable name after foreach")?;
        self.expect(TokenKind::In, "expected 'in' after foreach variable")?;
        let iterable = self.parse_expr()?;
        self.expect(TokenKind::LBrace, "expected '{' after foreach iterable")?;
        let mut body = Vec::new();
        while !self.matches(TokenKind::RBrace) {
            body.push(self.parse_stmt()?);
        }
        self.expect(TokenKind::RBrace, "expected '}' after foreach body")?;
        Ok(Stmt::ForEach {
            name,
            iterable,
            body,
            span: start,
        })
    }

    fn parse_assert(&mut self) -> Result<Stmt, Diagnostic> {
        let start = self.advance_with_span();
        self.expect(TokenKind::LParen, "expected '(' after assert")?;
        let cond = self.parse_expr()?;
        let message = if self.matches(TokenKind::Comma) {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.expect(TokenKind::RParen, "expected ')' after assert")?;
        self.expect(TokenKind::Semicolon, "expected ';' after assert")?;
        Ok(Stmt::Assert {
            cond,
            message,
            span: start,
        })
    }

    fn parse_panic(&mut self) -> Result<Stmt, Diagnostic> {
        let start = self.advance_with_span();
        self.expect(TokenKind::LParen, "expected '(' after panic")?;
        let message = self.parse_expr()?;
        self.expect(TokenKind::RParen, "expected ')' after panic")?;
        self.expect(TokenKind::Semicolon, "expected ';' after panic")?;
        Ok(Stmt::Panic {
            message,
            span: start,
        })
    }

    fn parse_type(&mut self) -> Result<TypeName, Diagnostic> {
        if self.matches(TokenKind::Ampersand) {
            self.advance();
            let mutable = if self.matches(TokenKind::Mut) {
                self.advance();
                true
            } else {
                false
            };
            let inner = self.parse_type()?;
            return Ok(TypeName::Ref {
                mutable,
                inner: Box::new(inner),
            });
        }

        if self.matches(TokenKind::LBracket) {
            self.advance();
            let elem = self.parse_type()?;
            self.expect(TokenKind::Semicolon, "expected ';' in array type")?;
            let len_literal = self.expect_number("expected array length")?;
            self.expect(TokenKind::RBracket, "expected ']' after array type")?;
            let len = parse_array_len(&len_literal)?;
            return Ok(TypeName::Array {
                elem: Box::new(elem),
                len,
            });
        }

        let name = self.expect_ident("expected type name")?;
        if name == "dict" {
            self.expect(TokenKind::Less, "expected '<' in dict type")?;
            let key_ty = self.parse_type()?;
            self.expect(TokenKind::Comma, "expected ',' in dict type")?;
            let value_ty = self.parse_type()?;
            self.expect(TokenKind::Greater, "expected '>' in dict type")?;
            return Ok(TypeName::Dict {
                key: Box::new(key_ty),
                value: Box::new(value_ty),
            });
        }
        if name == "list" {
            self.expect(TokenKind::Less, "expected '<' in list type")?;
            let elem = self.parse_type()?;
            self.expect(TokenKind::Greater, "expected '>' in list type")?;
            return Ok(TypeName::List {
                elem: Box::new(elem),
            });
        }
        if name == "map" {
            self.expect(TokenKind::Less, "expected '<' in map type")?;
            let key = self.parse_type()?;
            self.expect(TokenKind::Comma, "expected ',' in map type")?;
            let value = self.parse_type()?;
            self.expect(TokenKind::Greater, "expected '>' in map type")?;
            return Ok(TypeName::Map {
                key: Box::new(key),
                value: Box::new(value),
            });
        }
        let ty = match name.as_str() {
            "string" => TypeName::String,
            "Path" => TypeName::Path,
            "File" => TypeName::File,
            "Thread" => TypeName::Thread,
            "byte" => TypeName::Byte,
            "char" => TypeName::Char,
            "bool" => TypeName::Bool,
            "u8" => TypeName::Int {
                signed: false,
                bits: 8,
            },
            "u16" => TypeName::Int {
                signed: false,
                bits: 16,
            },
            "u32" => TypeName::Int {
                signed: false,
                bits: 32,
            },
            "u64" => TypeName::Int {
                signed: false,
                bits: 64,
            },
            "u128" => TypeName::Int {
                signed: false,
                bits: 128,
            },
            "i8" => TypeName::Int {
                signed: true,
                bits: 8,
            },
            "i16" => TypeName::Int {
                signed: true,
                bits: 16,
            },
            "i32" => TypeName::Int {
                signed: true,
                bits: 32,
            },
            "i64" => TypeName::Int {
                signed: true,
                bits: 64,
            },
            "i128" => TypeName::Int {
                signed: true,
                bits: 128,
            },
            "usize" => TypeName::Int {
                signed: false,
                bits: 64,
            },
            "isize" => TypeName::Int {
                signed: true,
                bits: 64,
            },
            "f32" => TypeName::Float { bits: 32 },
            "f64" => TypeName::Float { bits: 64 },
            other => TypeName::Struct(other.to_string()),
        };
        if self.matches(TokenKind::Less) {
            if !matches!(ty, TypeName::Struct(_)) {
                let token = self.peek();
                return Err(Diagnostic::new(
                    "only nominal types accept generic arguments",
                    token.line,
                    token.column,
                ));
            }
            self.advance();
            let mut args = Vec::new();
            if self.matches(TokenKind::Greater) {
                let token = self.peek();
                return Err(Diagnostic::new(
                    "generic type requires at least one argument",
                    token.line,
                    token.column,
                ));
            }
            loop {
                args.push(self.parse_type()?);
                if self.matches(TokenKind::Comma) {
                    self.advance();
                    continue;
                }
                break;
            }
            self.expect(
                TokenKind::Greater,
                "expected '>' after generic type arguments",
            )?;
            return Ok(TypeName::Applied { name, args });
        }
        Ok(ty)
    }

    fn parse_expr(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_logical_or()
    }

    fn parse_logical_or(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_logical_and()?;
        while self.matches(TokenKind::PipePipe) {
            let op_span = self.advance_with_span();
            let right = self.parse_logical_and()?;
            expr = Expr::Binary {
                op: BinaryOp::Or,
                left: Box::new(expr),
                right: Box::new(right),
                span: op_span,
            };
        }
        Ok(expr)
    }

    fn parse_logical_and(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_comparison()?;
        while self.matches(TokenKind::AmpAmp) {
            let op_span = self.advance_with_span();
            let right = self.parse_comparison()?;
            expr = Expr::Binary {
                op: BinaryOp::And,
                left: Box::new(expr),
                right: Box::new(right),
                span: op_span,
            };
        }
        Ok(expr)
    }

    fn parse_comparison(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_bit_or()?;
        while self.matches(TokenKind::EqualEqual)
            || self.matches(TokenKind::BangEqual)
            || self.matches(TokenKind::Less)
            || self.matches(TokenKind::LessEqual)
            || self.matches(TokenKind::Greater)
            || self.matches(TokenKind::GreaterEqual)
        {
            let op_span = self.advance_with_span();
            let op = match &self.tokens[self.pos - 1].kind {
                TokenKind::EqualEqual => BinaryOp::Eq,
                TokenKind::BangEqual => BinaryOp::Ne,
                TokenKind::Less => BinaryOp::Lt,
                TokenKind::LessEqual => BinaryOp::Le,
                TokenKind::Greater => BinaryOp::Gt,
                TokenKind::GreaterEqual => BinaryOp::Ge,
                _ => unreachable!(),
            };
            let right = self.parse_bit_or()?;
            expr = Expr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
                span: op_span,
            };
        }
        Ok(expr)
    }

    fn parse_bit_or(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_bit_xor()?;
        while self.matches(TokenKind::Pipe) {
            let op_span = self.advance_with_span();
            let right = self.parse_bit_xor()?;
            expr = Expr::Binary {
                op: BinaryOp::BitOr,
                left: Box::new(expr),
                right: Box::new(right),
                span: op_span,
            };
        }
        Ok(expr)
    }

    fn parse_bit_xor(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_bit_and()?;
        while self.matches(TokenKind::Caret) {
            let op_span = self.advance_with_span();
            let right = self.parse_bit_and()?;
            expr = Expr::Binary {
                op: BinaryOp::BitXor,
                left: Box::new(expr),
                right: Box::new(right),
                span: op_span,
            };
        }
        Ok(expr)
    }

    fn parse_bit_and(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_shift()?;
        while self.matches(TokenKind::Ampersand) {
            let op_span = self.advance_with_span();
            let right = self.parse_shift()?;
            expr = Expr::Binary {
                op: BinaryOp::BitAnd,
                left: Box::new(expr),
                right: Box::new(right),
                span: op_span,
            };
        }
        Ok(expr)
    }

    fn parse_shift(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_add_sub()?;
        while self.matches(TokenKind::ShiftLeft) || self.matches(TokenKind::ShiftRight) {
            let op_span = self.advance_with_span();
            let op = if self.tokens[self.pos - 1].kind == TokenKind::ShiftLeft {
                BinaryOp::Shl
            } else {
                BinaryOp::Shr
            };
            let right = self.parse_add_sub()?;
            expr = Expr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
                span: op_span,
            };
        }
        Ok(expr)
    }

    fn parse_add_sub(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_mul_div()?;
        while self.matches(TokenKind::Plus) || self.matches(TokenKind::Minus) {
            let op_span = self.advance_with_span();
            let op = if self.tokens[self.pos - 1].kind == TokenKind::Plus {
                BinaryOp::Add
            } else {
                BinaryOp::Sub
            };
            let right = self.parse_mul_div()?;
            expr = Expr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
                span: op_span,
            };
        }
        Ok(expr)
    }

    fn parse_mul_div(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_unary()?;
        while self.matches(TokenKind::Star)
            || self.matches(TokenKind::Slash)
            || self.matches(TokenKind::Percent)
        {
            let op_span = self.advance_with_span();
            let op = match self.tokens[self.pos - 1].kind {
                TokenKind::Star => BinaryOp::Mul,
                TokenKind::Slash => BinaryOp::Div,
                TokenKind::Percent => BinaryOp::Mod,
                _ => unreachable!(),
            };
            let right = self.parse_unary()?;
            expr = Expr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
                span: op_span,
            };
        }
        Ok(expr)
    }

    fn parse_unary(&mut self) -> Result<Expr, Diagnostic> {
        if self.matches(TokenKind::Plus) {
            let span = self.advance_with_span();
            let expr = self.parse_unary()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Plus,
                expr: Box::new(expr),
                span,
            });
        }
        if self.matches(TokenKind::Minus) {
            let span = self.advance_with_span();
            let expr = self.parse_unary()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Neg,
                expr: Box::new(expr),
                span,
            });
        }
        if self.matches(TokenKind::Bang) {
            let span = self.advance_with_span();
            let expr = self.parse_unary()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(expr),
                span,
            });
        }
        if self.matches(TokenKind::Ampersand) {
            let span = self.advance_with_span();
            let mutable = if self.matches(TokenKind::Mut) {
                self.advance();
                true
            } else {
                false
            };
            let expr = self.parse_unary()?;
            return Ok(Expr::Unary {
                op: if mutable {
                    UnaryOp::RefMut
                } else {
                    UnaryOp::Ref
                },
                expr: Box::new(expr),
                span,
            });
        }

        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.matches(TokenKind::Dot) {
                let span = self.advance_with_span();
                let name = self.expect_ident("expected field or method name after '.'")?;
                if self.matches(TokenKind::LParen) {
                    self.advance();
                    let mut args = Vec::new();
                    if !self.matches(TokenKind::RParen) {
                        loop {
                            args.push(self.parse_expr()?);
                            if self.matches(TokenKind::Comma) {
                                self.advance();
                                if self.matches(TokenKind::RParen) {
                                    break;
                                }
                                continue;
                            }
                            break;
                        }
                    }
                    self.expect(TokenKind::RParen, "expected ')' after method call")?;
                    expr = Expr::MethodCall {
                        receiver: Box::new(expr),
                        name,
                        args,
                        span,
                    };
                } else {
                    expr = Expr::FieldAccess {
                        base: Box::new(expr),
                        field: name,
                        span,
                    };
                }
                continue;
            }
            if self.matches(TokenKind::LBracket) {
                let span = self.advance_with_span();
                let index = self.parse_expr()?;
                self.expect(TokenKind::RBracket, "expected ']' after index")?;
                expr = Expr::Index {
                    base: Box::new(expr),
                    index: Box::new(index),
                    span,
                };
                continue;
            }
            break;
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr, Diagnostic> {
        match &self.peek().kind {
            TokenKind::True => {
                let span = self.peek_span();
                self.advance();
                Ok(Expr::Bool { value: true, span })
            }
            TokenKind::False => {
                let span = self.peek_span();
                self.advance();
                Ok(Expr::Bool { value: false, span })
            }
            TokenKind::String(value) => {
                let span = self.peek_span();
                let value = value.clone();
                self.advance();
                Ok(Expr::String { value, span })
            }
            TokenKind::Char(value) => {
                let span = self.peek_span();
                let value = *value;
                self.advance();
                Ok(Expr::Char { value, span })
            }
            TokenKind::Number(literal) => {
                let span = self.peek_span();
                let literal = literal.clone();
                self.advance();
                Ok(Expr::Number { literal, span })
            }
            TokenKind::Match => self.parse_match_expr(),
            TokenKind::Ident(name) => {
                let span = self.peek_span();
                let name = name.clone();
                self.advance();
                if self.matches(TokenKind::LParen) {
                    self.advance();
                    let mut args = Vec::new();
                    if !self.matches(TokenKind::RParen) {
                        loop {
                            args.push(self.parse_expr()?);
                            if self.matches(TokenKind::Comma) {
                                self.advance();
                                if self.matches(TokenKind::RParen) {
                                    break;
                                }
                                continue;
                            }
                            break;
                        }
                    }
                    self.expect(TokenKind::RParen, "expected ')' after call")?;
                    return Ok(Expr::Call { name, args, span });
                }
                if self.matches(TokenKind::LBrace) && self.looks_like_struct_init() {
                    return self.parse_struct_init(name, span);
                }
                if self.matches(TokenKind::ColonColon) {
                    self.advance();
                    let member = self.expect_ident("expected member after '::'")?;
                    if self.matches(TokenKind::LParen) {
                        self.advance();
                        let mut args = Vec::new();
                        if !self.matches(TokenKind::RParen) {
                            loop {
                                args.push(self.parse_expr()?);
                                if self.matches(TokenKind::Comma) {
                                    self.advance();
                                    if self.matches(TokenKind::RParen) {
                                        break;
                                    }
                                    continue;
                                }
                                break;
                            }
                        }
                        self.expect(TokenKind::RParen, "expected ')' after associated call")?;
                        return Ok(Expr::QualifiedCall {
                            owner: name,
                            member,
                            args,
                            span,
                        });
                    }
                    if self.matches(TokenKind::LBrace) && self.looks_like_struct_init() {
                        return self.parse_enum_struct_variant(name, member, span);
                    }
                    return Ok(Expr::EnumVariant {
                        enum_name: name,
                        variant: member,
                        span,
                    });
                }
                Ok(Expr::Ident { name, span })
            }
            TokenKind::LBracket => self.parse_array_lit(),
            TokenKind::LBrace => self.parse_dict_lit(),
            TokenKind::LParen => {
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(TokenKind::RParen, "expected ')' after expression")?;
                Ok(expr)
            }
            _ => {
                let token = self.peek();
                Err(Diagnostic::new(
                    "expected expression",
                    token.line,
                    token.column,
                ))
            }
        }
    }

    fn parse_match_expr(&mut self) -> Result<Expr, Diagnostic> {
        let span = self.advance_with_span();
        let value = Box::new(self.parse_expr()?);
        self.expect(TokenKind::LBrace, "expected '{' after match value")?;
        let mut arms = Vec::new();
        while !self.matches(TokenKind::RBrace) {
            if self.matches(TokenKind::Eof) {
                let token = self.peek();
                return Err(Diagnostic::new(
                    "unterminated match expression",
                    token.line,
                    token.column,
                ));
            }
            let arm_span = self.peek_span();
            let pattern = self.parse_match_pattern()?;
            self.expect(TokenKind::FatArrow, "expected '=>' after match pattern")?;
            let expr = self.parse_expr()?;
            arms.push(MatchArm {
                pattern,
                expr,
                span: arm_span,
            });
            if self.matches(TokenKind::Comma) {
                self.advance();
            } else if !self.matches(TokenKind::RBrace) {
                let token = self.peek();
                return Err(Diagnostic::new(
                    "expected ',' between match arms",
                    token.line,
                    token.column,
                ));
            }
        }
        self.expect(TokenKind::RBrace, "expected '}' after match arms")?;
        if arms.is_empty() {
            return Err(Diagnostic::new(
                "match expression must contain at least one arm",
                span.line,
                span.column,
            ));
        }
        Ok(Expr::Match { value, arms, span })
    }

    fn parse_match_pattern(&mut self) -> Result<MatchPattern, Diagnostic> {
        let span = self.peek_span();
        let enum_name = self.expect_ident("expected enum pattern or '_'")?;
        if enum_name == "_" {
            return Ok(MatchPattern::Wildcard { span });
        }
        self.expect(TokenKind::ColonColon, "expected '::' in enum pattern")?;
        let variant = self.expect_ident("expected variant name in enum pattern")?;
        if self.matches(TokenKind::LParen) {
            self.advance();
            let mut bindings = Vec::new();
            if !self.matches(TokenKind::RParen) {
                loop {
                    let binding = self.expect_ident("expected binding or '_' in tuple pattern")?;
                    bindings.push((binding != "_").then_some(binding));
                    if self.matches(TokenKind::Comma) {
                        self.advance();
                        if self.matches(TokenKind::RParen) {
                            break;
                        }
                        continue;
                    }
                    break;
                }
            }
            self.expect(TokenKind::RParen, "expected ')' after tuple pattern")?;
            return Ok(MatchPattern::EnumTuple {
                enum_name,
                variant,
                bindings,
                span,
            });
        }
        if self.matches(TokenKind::LBrace) {
            self.advance();
            let mut fields = Vec::new();
            if !self.matches(TokenKind::RBrace) {
                loop {
                    let field_span = self.peek_span();
                    let name = self.expect_ident("expected field name in named pattern")?;
                    let binding = if self.matches(TokenKind::Colon) {
                        self.advance();
                        let binding =
                            self.expect_ident("expected binding or '_' after pattern field")?;
                        (binding != "_").then_some(binding)
                    } else {
                        Some(name.clone())
                    };
                    fields.push(MatchNamedFieldPattern {
                        name,
                        binding,
                        span: field_span,
                    });
                    if self.matches(TokenKind::Comma) {
                        self.advance();
                        if self.matches(TokenKind::RBrace) {
                            break;
                        }
                        continue;
                    }
                    break;
                }
            }
            self.expect(TokenKind::RBrace, "expected '}' after named pattern")?;
            return Ok(MatchPattern::EnumNamed {
                enum_name,
                variant,
                fields,
                span,
            });
        }
        Ok(MatchPattern::EnumUnit {
            enum_name,
            variant,
            span,
        })
    }

    fn parse_array_lit(&mut self) -> Result<Expr, Diagnostic> {
        let start = self.advance_with_span();
        let mut elems = Vec::new();
        if !self.matches(TokenKind::RBracket) {
            loop {
                elems.push(self.parse_expr()?);
                if self.matches(TokenKind::Comma) {
                    self.advance();
                    if self.matches(TokenKind::RBracket) {
                        break;
                    }
                    continue;
                }
                break;
            }
        }
        self.expect(TokenKind::RBracket, "expected ']' after array literal")?;
        Ok(Expr::ArrayLit { elems, span: start })
    }

    fn parse_struct_init(&mut self, name: String, span: Span) -> Result<Expr, Diagnostic> {
        self.expect(TokenKind::LBrace, "expected '{' in struct literal")?;
        let mut fields = Vec::new();
        if !self.matches(TokenKind::RBrace) {
            loop {
                let field_span = self.peek_span();
                let field_name = self.expect_ident("expected field name")?;
                self.expect(TokenKind::Colon, "expected ':' after field name")?;
                let expr = self.parse_expr()?;
                fields.push(StructInitField {
                    name: field_name,
                    expr,
                    span: field_span,
                });
                if self.matches(TokenKind::Comma) {
                    self.advance();
                    if self.matches(TokenKind::RBrace) {
                        break;
                    }
                    continue;
                }
                break;
            }
        }
        self.expect(TokenKind::RBrace, "expected '}' after struct literal")?;
        Ok(Expr::StructInit { name, fields, span })
    }

    fn parse_enum_struct_variant(
        &mut self,
        enum_name: String,
        variant: String,
        span: Span,
    ) -> Result<Expr, Diagnostic> {
        self.expect(TokenKind::LBrace, "expected '{' in enum payload")?;
        let mut fields = Vec::new();
        if !self.matches(TokenKind::RBrace) {
            loop {
                let field_span = self.peek_span();
                let field_name = self.expect_ident("expected enum payload field name")?;
                self.expect(TokenKind::Colon, "expected ':' after enum payload field")?;
                fields.push(StructInitField {
                    name: field_name,
                    expr: self.parse_expr()?,
                    span: field_span,
                });
                if self.matches(TokenKind::Comma) {
                    self.advance();
                    if self.matches(TokenKind::RBrace) {
                        break;
                    }
                    continue;
                }
                break;
            }
        }
        self.expect(TokenKind::RBrace, "expected '}' after enum payload")?;
        Ok(Expr::EnumStructVariant {
            enum_name,
            variant,
            fields,
            span,
        })
    }

    fn parse_dict_lit(&mut self) -> Result<Expr, Diagnostic> {
        let start = self.advance_with_span();
        let mut entries = Vec::new();
        if !self.matches(TokenKind::RBrace) {
            loop {
                let entry_span = self.peek_span();
                let key = match &self.peek().kind {
                    TokenKind::String(value) => {
                        let value = value.clone();
                        self.advance();
                        value
                    }
                    _ => {
                        let token = self.peek();
                        return Err(Diagnostic::new(
                            "dictionary keys must be string literals",
                            token.line,
                            token.column,
                        ));
                    }
                };
                self.expect(TokenKind::Colon, "expected ':' after dictionary key")?;
                let value = self.parse_expr()?;
                entries.push(DictEntry {
                    key,
                    value,
                    span: entry_span,
                });
                if self.matches(TokenKind::Comma) {
                    self.advance();
                    if self.matches(TokenKind::RBrace) {
                        break;
                    }
                    continue;
                }
                break;
            }
        }
        self.expect(TokenKind::RBrace, "expected '}' after dictionary literal")?;
        Ok(Expr::DictLit {
            entries,
            span: start,
        })
    }

    fn expect(&mut self, kind: TokenKind, message: &str) -> Result<(), Diagnostic> {
        if self.matches(kind.clone()) {
            self.advance();
            Ok(())
        } else {
            let token = self.peek();
            Err(Diagnostic::new(message, token.line, token.column))
        }
    }

    fn expect_ident(&mut self, message: &str) -> Result<String, Diagnostic> {
        match &self.peek().kind {
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                Ok(name)
            }
            _ => {
                let token = self.peek();
                Err(Diagnostic::new(message, token.line, token.column))
            }
        }
    }

    fn expect_number(&mut self, message: &str) -> Result<String, Diagnostic> {
        match &self.peek().kind {
            TokenKind::Number(literal) => {
                let literal = literal.clone();
                self.advance();
                Ok(literal)
            }
            _ => {
                let token = self.peek();
                Err(Diagnostic::new(message, token.line, token.column))
            }
        }
    }

    fn expect_ident_text(&mut self, expected: &str, message: &str) -> Result<(), Diagnostic> {
        match &self.peek().kind {
            TokenKind::Ident(value) if value == expected => {
                self.advance();
                Ok(())
            }
            _ => {
                let token = self.peek();
                Err(Diagnostic::new(message, token.line, token.column))
            }
        }
    }

    fn expect_struct_member_sep(&mut self, message: &str) -> Result<(), Diagnostic> {
        if self.matches(TokenKind::Semicolon) || self.matches(TokenKind::Comma) {
            self.advance();
            Ok(())
        } else {
            let token = self.peek();
            Err(Diagnostic::new(message, token.line, token.column))
        }
    }

    fn matches(&self, kind: TokenKind) -> bool {
        self.peek().kind == kind
    }

    fn matches_ident(&self) -> bool {
        matches!(self.peek().kind, TokenKind::Ident(_))
    }

    fn matches_ident_text(&self, expected: &str) -> bool {
        matches!(&self.peek().kind, TokenKind::Ident(value) if value == expected)
    }

    fn peek(&self) -> &Token {
        self.tokens
            .get(self.pos)
            .unwrap_or_else(|| &self.tokens[self.tokens.len() - 1])
    }

    fn peek_n(&self, n: usize) -> &Token {
        self.tokens
            .get(self.pos + n)
            .unwrap_or_else(|| &self.tokens[self.tokens.len() - 1])
    }

    fn looks_like_struct_init(&self) -> bool {
        if !self.matches(TokenKind::LBrace) {
            return false;
        }
        match &self.peek_n(1).kind {
            TokenKind::RBrace => true,
            TokenKind::Ident(_) => matches!(self.peek_n(2).kind, TokenKind::Colon),
            _ => false,
        }
    }

    fn peek_span(&self) -> Span {
        let token = self.peek();
        Span::in_source(token.line, token.column, token.source_id)
    }

    fn advance_with_span(&mut self) -> Span {
        let span = self.peek_span();
        self.advance();
        span
    }

    fn advance(&mut self) {
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
    }
}

fn parse_array_len(literal: &str) -> Result<u64, Diagnostic> {
    let mut digits = String::new();
    for ch in literal.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else {
            break;
        }
    }
    if digits.is_empty() {
        return Err(Diagnostic::new("array length must be integer", 0, 0));
    }
    digits
        .parse::<u64>()
        .map_err(|_| Diagnostic::new("array length out of range", 0, 0))
}

#[cfg(test)]
#[path = "parser/tests.rs"]
mod tests;
