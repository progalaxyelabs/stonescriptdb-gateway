# StoneScriptDB Gateway Documentation

Welcome to the StoneScriptDB Gateway documentation. Choose your path based on what you need:

---

## 🚀 Getting Started

**New to the gateway?** Start here:

1. **[Quick Start Guide](QUICKSTART.md)** ⏱️ 5 minutes
   - Create your first schema
   - Register with gateway
   - Call your first function
   - Perfect for: First-time users

2. **[Integration Guide](INTEGRATION.md)** ⏱️ 15 minutes
   - StoneScriptPHP integration
   - Docker Compose setup
   - CI/CD patterns
   - Perfect for: Platform developers

3. **[Development Environment](DEV-ENVIRONMENT.md)** ⏱️ 30 minutes
   - Local VM setup with libvirt
   - PostgreSQL configuration
   - Gateway deployment
   - Perfect for: Contributors & local dev

---

## 📖 Reference Documentation

### Main Documentation

**[README (Main)](../README.md)** - Complete reference
- Features overview
- Schema structure (extensions, types, tables, functions, migrations)
- API endpoints (v1 and v2)
- Gateway tracking tables & checksums
- Environment variables
- StoneScriptPHP integration

### API Reference

**[API v2](API-V2.md)** - Multi-tenant platform management
- Platform registration
- Stored schemas
- On-demand database creation
- Migration workflows
- Perfect for: Multi-tenant SaaS platforms

### Architecture

**[High-Level Design (HLD)](../HLD.md)** - Technical design
- System architecture
- Design decisions & rationale
- Component interactions
- Database schema
- Perfect for: Architects & senior developers

---

## 🔬 Design Documents

**[Declarative Schema Design](DECLARATIVE_SCHEMA_PROPOSAL.md)** - Implementation roadmap
- Feature implementation status
- Design approach
- Migration from numbered migrations
- Perfect for: Contributors understanding architecture decisions

**[Documentation Audit](DOCUMENTATION-AUDIT.md)** - Meta-documentation
- Documentation inventory
- Redundancy analysis
- Reorganization recommendations
- Perfect for: Documentation maintainers

---

## 📊 Documentation by Use Case

| I want to... | Read this | Time |
|--------------|-----------|------|
| **Get started quickly** | [Quick Start](QUICKSTART.md) | 5 min |
| **Integrate my platform** | [Integration Guide](INTEGRATION.md) | 15 min |
| **Understand the architecture** | [HLD](../HLD.md) | 30 min |
| **Set up local development** | [Dev Environment](DEV-ENVIRONMENT.md) | 30 min |
| **Use the v2 API** | [API v2](API-V2.md) | 15 min |
| **Learn about schema structure** | [README: Schema Structure](../README.md#schema-tar-gz-structure) | 10 min |
| **Understand tracking tables** | [README: Gateway Tracking Tables](../README.md#gateway-tracking-tables) | 5 min |
| **Deploy to production** | [README: Production Deployment](../README.md#production-deployment-on-vm) | 20 min |
| **Configure Docker Compose** | [Integration: Docker Compose](INTEGRATION.md#docker-compose-integration) | 10 min |
| **Set up CI/CD** | [Integration: CI/CD](INTEGRATION.md#cicd-integration) | 15 min |
| **Troubleshoot issues** | [Integration: Troubleshooting](INTEGRATION.md#troubleshooting) | 5 min |

---

## 📚 Documentation Structure

```
stonescriptdb-gateway/
├── README.md                       # Main entry point (features, API, schema)
├── HLD.md                          # High-level design
└── docs/
    ├── README.md                   # This file (documentation hub)
    ├── QUICKSTART.md               # 5-minute getting started
    ├── INTEGRATION.md              # Platform integration guide
    ├── DEV-ENVIRONMENT.md          # Local development setup
    ├── API-V2.md                   # v2 API reference
    ├── DECLARATIVE_SCHEMA_PROPOSAL.md   # Design document
    └── DOCUMENTATION-AUDIT.md      # Documentation audit report
```

---

## 🎓 Learning Path

### Beginner Path
1. Read [Overview](../README.md#overview)
2. Follow [Quick Start](QUICKSTART.md)
3. Explore [Integration Guide](INTEGRATION.md)

### Developer Path
1. Read [README](../README.md) (full)
2. Set up [Dev Environment](DEV-ENVIRONMENT.md)
3. Review [HLD](../HLD.md) for architecture
4. Check [Declarative Schema Design](DECLARATIVE_SCHEMA_PROPOSAL.md)

### Architect Path
1. Read [Overview](../README.md#overview)
2. Study [HLD](../HLD.md) thoroughly
3. Review [API v2](API-V2.md) for multi-tenant patterns
4. Check [Declarative Schema Design](DECLARATIVE_SCHEMA_PROPOSAL.md)

### Integrator Path
1. Skim [README Features](../README.md#features)
2. Follow [Quick Start](QUICKSTART.md)
3. Deep dive [Integration Guide](INTEGRATION.md)
4. Reference [API v2](API-V2.md) as needed

---

## 📝 Documentation Metrics

- **Total:** ~3,350 lines across 8 files (after optimization)
- **Last major update:** 2026-01-05 (VM architecture migration)
- **Last audit:** 2026-01-05 (see [DOCUMENTATION-AUDIT.md](DOCUMENTATION-AUDIT.md))
- **Status:** ✅ All docs up-to-date

### Recent Changes
- ✅ Migrated from host-based to VM-based deployment
- ✅ Refactored INTEGRATION.md (reduced 50% redundancy)
- ✅ Added navigation bars to all documents
- ✅ Fixed localhost references → `<VM_IP>` placeholder
- ✅ Updated DECLARATIVE_SCHEMA_PROPOSAL status

---

## 💡 Tips for Finding Information

1. **Use the navigation bar** at the top of each document
2. **Start with Quick Start** if you're new
3. **README is the source of truth** for schema structure details
4. **HLD explains "why"**, README explains "what" and "how"
5. **INTEGRATION focuses on patterns**, README focuses on features
6. **Search tip:** Most docs link back to README for detailed explanations

---

## 🔗 External Resources

- **GitHub Repository:** https://github.com/progalaxyelabs/stonescriptdb-gateway
- **Issue Tracker:** https://github.com/progalaxyelabs/stonescriptdb-gateway/issues
- **Releases:** https://github.com/progalaxyelabs/stonescriptdb-gateway/releases
- **License:** MIT

---

## 📞 Getting Help

- **Found a bug?** [Open an issue](https://github.com/progalaxyelabs/stonescriptdb-gateway/issues)
- **Have a question?** Check [Troubleshooting](INTEGRATION.md#troubleshooting) first
- **Want to contribute?** Read [HLD](../HLD.md) and [Declarative Schema Design](DECLARATIVE_SCHEMA_PROPOSAL.md)

---

**Last updated:** 2026-01-05
